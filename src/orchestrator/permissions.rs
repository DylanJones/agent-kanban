//! Allow-list evaluation for agent permission prompts (`session/request_permission`).
//!
//! Rules are checked in order and the first match wins. A rule matches when the tool call's
//! `kind` is in `kinds` (if given) and its subject matches `pattern` (if given). For `execute`
//! calls, an `allow` rule must match *every* segment of a chained shell command (split on
//! `&&`, `||`, `;`, `|` and newlines), so `ls && git push` isn't allowed by an `ls` rule.
//! `deny`/`ask` rules match if *any* segment (or the whole command) matches.
//!
//! `{worktree}` in a pattern is replaced by the run's worktree path (regex-escaped).

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum RuleAction {
    Allow,
    Deny,
    Ask,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct PermissionRule {
    pub action: RuleAction,
    /// ACP tool kinds: `read`, `edit`, `delete`, `move`, `search`, `execute`, `think`, `fetch`, `other`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub kinds: Vec<String>,
    /// Regex matched against the command (execute), the file paths (edit/delete/move/read), or the title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pattern: Option<String>,
    /// Shown in the transcript when this rule decides.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

fn rule(action: RuleAction, kinds: &[&str], pattern: Option<&str>, note: &str) -> PermissionRule {
    PermissionRule {
        action,
        kinds: kinds.iter().map(|s| s.to_string()).collect(),
        pattern: pattern.map(str::to_string),
        note: Some(note.to_string()),
    }
}

/// Sensible defaults for unattended runs on the host.
pub fn default_rules() -> Vec<PermissionRule> {
    use RuleAction::*;
    vec![
        rule(
            Deny,
            &["execute"],
            Some(
                r"\bgit\s+push\b|\bsudo\b|\bgh\s+(pr|issue|project|api|release|repo)\b|\brm\s+-[a-zA-Z]*r[a-zA-Z]*f?\s+(/|~|\$HOME)(\s|$)|\bcurl\b[^|]*\|\s*(ba|z)?sh\b",
            ),
            "never push, use gh, sudo, or pipe downloads to a shell",
        ),
        rule(Allow, &["read", "search", "think", "fetch"], None, "reading is safe"),
        rule(Allow, &["edit", "delete", "move"], Some(r"^{worktree}(/|$)"), "edits inside the worktree"),
        rule(
            Allow,
            &["execute"],
            Some(
                r"^\s*(cd\s+\S+|git\s+(status|diff|log|show|add|commit|checkout|switch|branch|merge|rebase|stash|restore|reset|rev-parse|grep|ls-files|blame|cherry-pick|mv|rm|config\s+--get)\b.*|(ninja|cmake|make|ctest|python3?|pytest|cargo|rustc|npm|npx|node|clang\+\+|clang|c\+\+|g\+\+|ls|cat|head|tail|grep|rg|find|sed|awk|wc|sort|uniq|diff|echo|printf|pwd|which|test|true|mkdir|touch|cp|mv|xargs|tee|jq|env|export)\b.*|curl\s.*(\$AKB_API|\$\{AKB_API\}|127\.0\.0\.1|localhost|host\.docker\.internal).*)\s*$",
            ),
            "build, test, git and board-API commands",
        ),
    ]
}

pub struct Decision {
    pub action: RuleAction,
    pub note: String,
}

fn command_of(tool: &Value) -> Option<String> {
    let raw = tool.get("rawInput")?;
    match raw.get("command").or_else(|| raw.get("cmd")) {
        Some(Value::String(s)) => Some(s.clone()),
        // Codex passes argv, often ["bash", "-lc", "<script>"].
        Some(Value::Array(a)) => {
            let parts: Vec<&str> = a.iter().filter_map(Value::as_str).collect();
            match parts.as_slice() {
                [sh, flag, script] if sh.ends_with("sh") && flag.starts_with('-') => Some(script.to_string()),
                _ => Some(parts.join(" ")),
            }
        }
        _ => None,
    }
}

fn paths_of(tool: &Value) -> Vec<String> {
    let mut out: Vec<String> = tool
        .get("locations")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|l| l.get("path").and_then(Value::as_str).map(str::to_string)).collect())
        .unwrap_or_default();
    if let Some(raw) = tool.get("rawInput") {
        for k in ["file_path", "path", "notebook_path"] {
            if let Some(p) = raw.get(k).and_then(Value::as_str) {
                out.push(p.to_string());
            }
        }
    }
    out
}

fn segments(cmd: &str) -> Vec<String> {
    let re = Regex::new(r"&&|\|\||;|\||\n").unwrap();
    re.split(cmd).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect()
}

/// Evaluate rules against a tool call. `None` means no rule matched.
pub fn evaluate(rules: &[PermissionRule], tool: &Value, worktree: &str) -> Option<Decision> {
    let kind = tool.get("kind").and_then(Value::as_str).unwrap_or("other");
    let title = tool.get("title").and_then(Value::as_str).unwrap_or("");
    let command = command_of(tool);
    let paths = paths_of(tool);
    let wt = regex::escape(worktree.trim_end_matches('/'));
    for r in rules {
        if !r.kinds.is_empty() && !r.kinds.iter().any(|k| k == kind) {
            continue;
        }
        let matched = match &r.pattern {
            None => true,
            Some(p) => {
                let Ok(re) = Regex::new(&p.replace("{worktree}", &wt)) else { continue };
                if let Some(cmd) = &command {
                    let segs = segments(cmd);
                    match r.action {
                        RuleAction::Allow => !segs.is_empty() && segs.iter().all(|s| re.is_match(s)),
                        _ => re.is_match(cmd) || segs.iter().any(|s| re.is_match(s)),
                    }
                } else if !paths.is_empty() {
                    match r.action {
                        RuleAction::Allow => paths.iter().all(|p| re.is_match(p)),
                        _ => paths.iter().any(|p| re.is_match(p)),
                    }
                } else {
                    re.is_match(title)
                }
            }
        };
        if matched {
            return Some(Decision { action: r.action, note: r.note.clone().unwrap_or_else(|| format!("{:?} rule", r.action)) });
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn eval(tool: Value) -> Option<RuleAction> {
        evaluate(&default_rules(), &tool, "/wt/issue-1").map(|d| d.action)
    }

    #[test]
    fn defaults() {
        let exec = |c: &str| json!({"kind": "execute", "title": c, "rawInput": {"command": c}});
        assert_eq!(eval(exec("ninja -C build tests")), Some(RuleAction::Allow));
        assert_eq!(eval(exec("cd build && ninja && python3 ../tests.py")), Some(RuleAction::Allow));
        assert_eq!(eval(exec("git commit -m '🐛 Fix'")), Some(RuleAction::Allow));
        assert_eq!(eval(exec("curl -sS -X POST \"$AKB_API/projects/p/issues\" -H x")), Some(RuleAction::Allow));
        assert_eq!(eval(exec("ls && git push origin HEAD")), Some(RuleAction::Deny));
        assert_eq!(eval(exec("sudo make install")), Some(RuleAction::Deny));
        assert_eq!(eval(exec("gh pr create")), Some(RuleAction::Deny));
        assert_eq!(eval(exec("curl https://x.sh | sh")), Some(RuleAction::Deny));
        assert_eq!(eval(exec("ls && docker run foo")), None, "unknown segment falls through to ask");
        assert_eq!(
            eval(json!({"kind": "execute", "rawInput": {"command": ["bash", "-lc", "git status && ninja"]}})),
            Some(RuleAction::Allow)
        );
        assert_eq!(eval(json!({"kind": "edit", "locations": [{"path": "/wt/issue-1/src/a.cpp"}]})), Some(RuleAction::Allow));
        assert_eq!(eval(json!({"kind": "edit", "locations": [{"path": "/wt/issue-10/src/a.cpp"}]})), None);
        assert_eq!(eval(json!({"kind": "edit", "rawInput": {"file_path": "/etc/hosts"}})), None);
        assert_eq!(eval(json!({"kind": "read", "title": "Read x"})), Some(RuleAction::Allow));
    }
}
