# 🗂️ agent-kanban

> [!WARNING]
> **AI Level 5: fully vibe-coded, with human testing.**

A local kanban board that runs coding agents: Claude Code, Codex, OpenCode, or any
[Agent Client Protocol](https://agentclientprotocol.com) adapter. It moves issues through a
triage → fix → review → merge workflow. The board is the source of truth for issues, pull
requests, inline reviews and comment threads. GitHub is optional: you can import from it once
and mirror changes back.

- **Board.** Four columns: Backlog / In progress / In review / Done. State badges show
  *Triage*, *Ready*, *Changes requested*, *Merge conflict* and *Ready to merge*. A
  *Needs decision* hold can pause any card.
- **Agents over ACP.** You pick which agent handles each role (triage, fix, review, merge
  prep) per project. The defaults are Claude Code for triage, fix and merge prep, and Codex for
  review. The UI shows live transcripts, and you can stop a run or message it mid-run.
- **Concurrency.** There's a global limit on simultaneous agents, plus optional limits per
  project and per agent.
- **Subscription limits.** When Claude Code or Codex hits a usage limit, every agent sharing
  that subscription pauses. They resume on their own at the reported reset time. If no reset
  time is reported, a cheap probe checks periodically.
- **Worktrees.** Each issue gets its own git worktree and branch. Review and triage runs use
  throwaway detached worktrees. Worktrees are cleaned up when an issue is done, closed, or
  sent back to the backlog.
- **Containers (optional).** Agents can run in a per-project Docker image, with the worktree
  mounted.
- **REST API.** Documented with OpenAPI at `/api/docs`. Agents get a per-run token and a
  one-line way to file bugs they notice along the way.
- **PRs and merging.** Local PRs show diffs with inline, line-anchored review threads. Threads
  are re-anchored when new commits arrive, and marked outdated when the code they point at
  changes. A human merges (squash, merge commit or rebase) without touching anyone's
  checkout.

## Quick start

```sh
# Build (Rust 1.94+, Node 20+)
npm --prefix web install && npm --prefix web run build
cargo build --release

# Register a repository and import its GitHub issues/PRs/board
./target/release/agent-kanban add-project emojicode ~/workspace/emojicode \
    --base-branch master --github-repo DylanJones/emojicode --github-project DylanJones/1
./target/release/agent-kanban import-github --project emojicode

# Run it and open the printed login link
./target/release/agent-kanban serve
# …or reachable from other devices on your network (plain HTTP; login still required)
./target/release/agent-kanban serve --bind 0.0.0.0:7878
```

To keep it running in the background on macOS (it starts at login and restarts if it exits),
install it as a LaunchAgent:

```sh
scripts/service.sh install 0.0.0.0:7878   # or omit the bind for 127.0.0.1:7878
scripts/service.sh login-url              # the login link
cargo build --release && scripts/service.sh restart   # deploy a new build
scripts/service.sh status | logs | uninstall
```

It runs `target/release/agent-kanban` from this checkout and logs to
`~/.agent-kanban/logs/server.log`. Stopping or restarting it interrupts active runs, which
resume on the next start. Projects that use containers wait for Docker Desktop to be running before
dispatching.

The scheduler starts **off**. Turn it on with the switch in the header, or pass
`serve --scheduler`. Until then, agents only run when you press **Run** on an issue.

Before the first real run, open **Agents** and press **Test** on each agent. Test checks the
adapter starts and completes the ACP handshake; **+ prompt** also checks that the
subscription works. Then fill in the project's **agent instructions** and **worktree setup
script** under **Project**.

Data lives in `~/.agent-kanban/` (override with `--data-dir` or `AKB_DATA_DIR`):

- `db.sqlite` holds everything on the board.
- `secrets.toml` (mode 0600) holds the admin token and optional container credentials.
- `worktrees/` holds agent checkouts.
- `runs/*.stderr.log` holds adapter logs.

## Workflow

```
            ┌──────── triage agent ────────┐
 reported → triage → ready ─ fix agent → in_progress → in_review ─ review agent ─┬→ ready_to_merge → (human merges) → done
                       ↑                                   ↑                      └→ changes_requested ─ fix agent ┘
                       └──────── backlog / closed          └─ merge_conflict ─ merge-prep agent
 any state ──(agent or human asks a design question)──→ hold: needs_decision ──(human answers)──→ resumes
```

| State | Column | Who acts next |
|---|---|---|
| `triage` | Backlog | triage agent → `ready` / `backlog` / `closed` |
| `backlog`, `ready` | Backlog | fix agent picks up `ready` |
| `in_progress`, `changes_requested` | In progress | fix agent: commit, open/update PR, → `in_review` |
| `merge_conflict` | In progress | merge-prep agent merges the base branch, → `in_review` |
| `in_review` | In review | review agent posts a verdict at the PR head |
| `ready_to_merge` | In review | you: merge on the PR page |
| `done`, `closed` | Done | — |

Some transitions happen on their own:

- **New commits after approval.** The PR goes back to `in_review`.
- **Merge conflicts.** A PR that stops merging cleanly into the base moves to
  `merge_conflict`. It returns to review once the conflict is resolved.
- **Repeated failures.** Three failed runs in a row put the issue on hold as *stalled*.
- **Missing outcomes.** If a run ends without recording an outcome, the agent gets one nudge.
  If it still records nothing, the run counts as failed and retries back off (5m, 30m, 2h).

Humans can make any move (drag cards, or use the issue drawer). The **Inbox** collects open
decisions, permission prompts, stalled issues and the ready-to-merge queue.

## Agents

Agent definitions are ACP commands with arguments and environment variables, all editable
under **Agents**:

| Agent | Command | Modes used |
|---|---|---|
| Claude Code | `npx -y @agentclientprotocol/claude-agent-acp` | `auto` on host, `bypassPermissions` in containers |
| Codex | `npx -y @agentclientprotocol/codex-acp` | `agent` on host (network + repo `.git` writable), `agent-full-access` in containers |
| OpenCode | `opencode acp` | default |

Each run gets:

- **A worktree.** It is the session's working directory.
- **A prompt.** Rendered per role from minijinja templates you can edit under **Project →
  Prompt templates**. It includes the issue thread, decisions already made, open review
  threads and a cheat sheet for the board API.
- **Board tools over MCP.** An `agent-kanban` MCP server (`/mcp`, authenticated with the
  run's token) gives the agent `board_file_issue`, `board_comment`, `board_move_issue`,
  `board_request_decision`, `board_open_pr`, `board_get_diff`, `board_add_thread`,
  `board_submit_review` and more. The agent process makes these calls itself, outside its
  shell sandbox. That matters for Codex, whose workspace sandbox blocks network access from
  commands. Board tool calls are always permitted.
- **The REST API as a fallback.** `$AKB_API` and `$AKB_AUTH` (a run-scoped token), plus
  `AKB_PROJECT`, `AKB_ISSUE` and `AKB_RUN`, for curl.
- **The project's setup script, run before every run.** It runs in the container when
  containers are on. Keep it idempotent: it should be quick when nothing has changed, because
  a worktree can move between host and container toolchains.
- **Write access to the repo's shared `.git`** (as an extra session directory), so sandboxed
  agents can commit from a linked worktree.

### Model, effort and other session settings

Each agent reports the settings it supports over ACP, such as `model`, effort (`effort` for
Claude, `reasoning_effort` for Codex) and fast mode. The choices appear on the **Agents** page
after an agent's first run, or after you press **Load model & effort options**.

Settings apply at two levels:

- **Agent default** (Agents page): used for every run.
- **Per-role override** (Project → Agents): for example, triage on Sonnet at low effort and fix
  on Opus at xhigh.

They're set at the start of each session. A setting the agent doesn't accept is reported in the
run transcript rather than failing the run. Each run page shows the model and effort it
actually used.

### Permissions

Permissions work in two layers:

1. **The agent's own session mode**, selected over ACP when the session starts. Claude runs
   in **auto mode** on the host: Claude Code's classifier approves routine actions itself.
   Codex runs in `agent` mode, a workspace-write sandbox that allows network access and writes
   to the repository's `.git`. In containers they run as `bypassPermissions` and
   `agent-full-access`, because the container is the sandbox.
2. **Whatever the agent still asks about** (`session/request_permission`) is answered by the
   agent's policy. Each agent has one policy for the host and one for containers:
   - `allowlist`: the host default. Ordered rules, where the first match wins. If nothing
     matches, the prompt goes to the Inbox and waits up to 30 minutes for a human.
   - `auto_allow`: the container default. Approves everything.
   - `ask`: every prompt goes to the Inbox.
   - `deny`: rejects every prompt.

The default allowlist:

- **Denies** `git push`, `sudo`, `gh pr|issue|project|api`, `rm -rf /`, and piping downloads
  into a shell.
- **Allows** reads and searches, and edits inside the run's worktree.
- **Allows** build, test and git commands, and `curl` to the board API.

For chained shell commands (`a && b | c`), *every* segment must match an allow rule, so
`ls && git push` isn't allowed by the `ls` rule. You can edit the rules per agent (JSON) on the
**Agents** page:

```json
[{"action": "deny", "kinds": ["execute"], "pattern": "\\bgit\\s+push\\b"},
 {"action": "allow", "kinds": ["edit"], "pattern": "^{worktree}/"},
 {"action": "allow", "kinds": ["execute"], "pattern": "^(ninja|cmake|git (status|diff|commit))\\b.*"}]
```

## Subscription limits

Each agent belongs to a **limit group** (by default, its harness). When a run fails with a
usage-limit error, three things happen:

- The run is marked `rate_limited` and the issue isn't penalized.
- The whole group pauses until the reset time, plus a minute.
- When the group resumes, the issue is picked up again, and the new run is told to continue
  from the worktree's state.

Limits are detected from the adapters' structured error data: Claude's `errorKind` and
`_claude/rateLimit` metadata, and Codex's `codexErrorInfo`. The reset time is parsed from the
message ("resets 5pm (America/Los_Angeles)", "try again at 3:05 PM", "in 20 minutes"). If no
reset time is available, a one-line probe runs with exponential backoff. **Agents → Subscriptions**
shows the pause state and last usage snapshot, with **Resume**, **Probe** and **Pause** buttons.

## REST API

- Interactive docs: `http://127.0.0.1:7878/api/docs`
- OpenAPI: `/api/openapi.json`
- Agent guide with copy-paste curl commands: `GET /api/agent-guide`, also in
  [`docs/AGENT_API.md`](docs/AGENT_API.md)

Filing a bug in one call:

```sh
curl -sS -X POST "$AKB_API/projects/emojicode/issues" -H "Authorization: Bearer $AKB_AUTH" \
  -H 'Content-Type: text/plain' --data-binary $'Crash printing nested optional\nWhere: …\nRepro: …'
```

Auth works like this:

- **Humans:** the admin token (login link or bearer), or named API tokens from
  `agent-kanban token <name>`.
- **Agents:** a per-run token. It can file new issues, but it can only change the issue its
  run is bound to, and only in ways its role allows. Merging, decisions and settings are
  human-only.

## Containers

In **Project → Container sandbox**, provide a base Dockerfile with your toolchain, enable
containers and press **Build image** (or run `agent-kanban build-image --project <slug>`). The
Dockerfile shouldn't copy the source, because worktrees are mounted at run time.
[`examples/emojicode/`](examples/emojicode) has the emojicode toolchain image (Clang/LLVM
from apt.llvm.org, ccache, tree-sitter) and its host/container-agnostic setup script. agent-kanban adds an overlay on top that installs Node,
git, curl, ccache and the ACP adapters, and creates a non-root user with your uid.

Each run then executes `docker run --rm -i …`:

- **Mounts.** The worktree and the repo's `.git` are mounted at the same paths, so commits
  land in your repository. A shared ccache volume is mounted too.
- **API access.** The API is reachable at `host.docker.internal`.

Credentials are never shared with containers beyond what the agent needs:

- **Claude:** put a long-lived `claude_code_oauth_token` (from `claude setup-token`) in
  `secrets.toml`.
- **Codex:** `~/.codex/auth.json` is mounted.
- **GitHub and SSH:** never mounted. Pushes happen on the host.

## GitHub

`import-github` (or **Project → Import / re-sync**) brings in:

- the board's items and fields (Status, Priority, Size, Estimate, dates)
- labels
- all issues with their comments
- all PRs with comments, reviews and inline review threads
- local branches for open PRs, fetched if missing

It's idempotent, and local edits win over re-imports.

Optional mirroring, per project:

- push branches and merges
- open GitHub PRs
- sync the board's Status field (including "Needs decision")
- create GitHub issues for local ones
- post review verdicts

## Development

```sh
cargo test                       # unit + integration tests (scripted fake ACP agent, temp git repos)
npm --prefix web run dev         # Vite on :5173, proxies /api to :7878
cargo run -- serve --no-auth     # dev server without login
npm --prefix web run gen:api     # regenerate TypeScript types from the OpenAPI spec
```

`tests/fixtures/fake_acp_agent.py` is a scripted ACP agent. The integration tests use it to
drive the full workflow through the real HTTP API: triage, fix, review, changes, merge,
conflicts, limits, nudges and cancellation.
