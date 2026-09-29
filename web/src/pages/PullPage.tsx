import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import clsx from "clsx";
import { ArrowLeft, ArrowRight, CheckCircle2, ChevronDown, ChevronRight, CircleDot, ExternalLink, FileCode, GitCommitHorizontal, GitMerge, GitPullRequest, GitPullRequestClosed, MessageSquarePlus } from "lucide-react";
import { type ReactNode, useMemo, useState } from "react";
import { Diff, Hunk, type ChangeData, type FileData, getChangeKey, isDelete, isInsert, isNormal, parseDiff } from "react-diff-view";
import "react-diff-view/style/index.css";
import { Link, useParams } from "react-router";
import { type PullDetail, type Thread, api, client, unwrap } from "../api/client";
import { Avatar, Button, ErrorBox, Markdown, Page, Pill, Segmented, Switch, Tabs, TimeAgo, inputCls, selectCls, short } from "../components/ui";
import { CommentView } from "./IssueDrawer";

export function ThreadView({ t, compact }: { t: Thread; slug?: string; compact?: boolean }) {
  const qc = useQueryClient();
  const [reply, setReply] = useState("");
  const [open, setOpen] = useState(!t.resolved);
  const act = useMutation({
    mutationFn: (kind: "reply" | "resolve" | "unresolve") =>
      kind === "reply" ? api("POST", `/api/threads/${t.id}/replies`, { body: reply }) : api("POST", `/api/threads/${t.id}/${kind}`, {}),
    onSuccess: () => {
      setReply("");
      qc.invalidateQueries({ queryKey: ["pull"] });
    },
  });
  return (
    <div
      className={clsx(
        "m-2 overflow-hidden rounded-xl border bg-surface font-sans text-sm shadow-card",
        t.resolved ? "border-line" : t.severity === "blocking" ? "border-amber-500/40" : "border-line-strong",
      )}
    >
      <button className="flex w-full items-center gap-2 px-3 py-2 text-left text-xs text-fg-muted hover:bg-surface-2/60" onClick={() => setOpen(!open)}>
        {open ? <ChevronDown size={13} className="shrink-0" /> : <ChevronRight size={13} className="shrink-0" />}
        {compact && (
          <code className="min-w-0 shrink truncate font-mono text-fg">
            {t.path}:{t.line}
          </code>
        )}
        <Pill tone={t.severity === "blocking" ? "amber" : "neutral"} className="shrink-0">
          {t.severity}
        </Pill>
        {t.resolved && (
          <Pill tone="green" className="shrink-0">
            resolved
          </Pill>
        )}
        {t.outdated && <Pill className="shrink-0">outdated</Pill>}
        <span className="min-w-0 flex-1 truncate">{t.comments[0]?.body.slice(0, 90)}</span>
      </button>
      {open && (
        <div className="space-y-3 border-t border-line px-3 py-3">
          {t.outdated && t.diff_hunk && <pre className="overflow-x-auto rounded-lg bg-surface-2 p-2 font-mono text-[11px]">{t.diff_hunk}</pre>}
          {t.comments.map((c) => (
            <div key={c.id} className="flex items-start gap-2.5">
              <Avatar name={c.author_name} agent={c.author_kind === "agent"} size={22} />
              <div className="min-w-0 flex-1">
                <div className="mb-0.5 text-xs text-fg-subtle">
                  <b className="font-semibold text-fg">{c.author_name}</b> <TimeAgo iso={c.created_at} />
                </div>
                <Markdown>{c.body}</Markdown>
              </div>
            </div>
          ))}
          <div className="flex flex-col gap-2 sm:flex-row">
            <input
              className={clsx(inputCls, "sm:py-1.5")}
              placeholder="Reply…"
              value={reply}
              onChange={(e) => setReply(e.target.value)}
              onKeyDown={(e) => e.key === "Enter" && reply.trim() && act.mutate("reply")}
            />
            <div className="flex gap-2">
              <Button size="sm" className="h-9 flex-1 justify-center sm:h-auto" disabled={!reply.trim()} onClick={() => act.mutate("reply")}>
                Reply
              </Button>
              <Button size="sm" variant={t.resolved ? "default" : "soft"} className="h-9 flex-1 justify-center sm:h-auto" onClick={() => act.mutate(t.resolved ? "unresolve" : "resolve")}>
                {t.resolved ? "Unresolve" : "Resolve"}
              </Button>
            </div>
          </div>
          <ErrorBox error={act.error} />
        </div>
      )}
    </div>
  );
}

function NewThreadForm({ slug, n, path, line, side, onDone }: { slug: string; n: number; path: string; line: number; side: "LEFT" | "RIGHT"; onDone: () => void }) {
  const qc = useQueryClient();
  const [body, setBody] = useState("");
  const [severity, setSeverity] = useState("blocking");
  const m = useMutation({
    mutationFn: () => api("POST", `/api/projects/${slug}/pulls/${n}/threads`, { path, line, side, body, severity }),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["pull"] });
      onDone();
    },
  });
  return (
    <div className="m-2 space-y-2 rounded-xl border border-accent/40 bg-surface p-3 font-sans shadow-raised">
      <textarea
        autoFocus
        className={clsx(inputCls, "h-20 resize-y")}
        placeholder={`Comment on ${side === "RIGHT" ? "" : "old "}line ${line}…`}
        value={body}
        onChange={(e) => setBody(e.target.value)}
      />
      <div className="flex flex-wrap items-center gap-2">
        <Segmented
          size="sm"
          label="Severity"
          value={severity}
          onChange={setSeverity}
          options={[
            { value: "blocking", label: "Blocking" },
            { value: "nit", label: "Nit" },
          ]}
        />
        <span className="flex-1" />
        <Button size="sm" variant="ghost" onClick={onDone}>
          Cancel
        </Button>
        <Button size="sm" variant="primary" disabled={!body.trim()} onClick={() => m.mutate()}>
          Add comment
        </Button>
      </div>
      <ErrorBox error={m.error} />
    </div>
  );
}

function rightLine(c: ChangeData): number | null {
  if (isInsert(c)) return c.lineNumber;
  if (isNormal(c)) return c.newLineNumber;
  return null;
}

function FileView({ file, threads, slug, n, readOnly }: { file: FileData; threads: Thread[]; slug: string; n: number; readOnly?: boolean }) {
  const [collapsed, setCollapsed] = useState(false);
  const [draft, setDraft] = useState<{ key: string; line: number; side: "LEFT" | "RIGHT" } | null>(null);
  const path = file.type === "delete" ? file.oldPath : file.newPath;
  const widgets = useMemo(() => {
    const w: Record<string, ReactNode> = {};
    const byKey: Record<string, Thread[]> = {};
    for (const h of file.hunks) {
      for (const c of h.changes) {
        const key = getChangeKey(c);
        for (const t of threads) {
          if (t.outdated) continue;
          const hit = t.side === "LEFT" ? isDelete(c) && c.lineNumber === t.line : rightLine(c) === t.line;
          if (hit) (byKey[key] ??= []).push(t);
        }
      }
    }
    for (const [k, ts] of Object.entries(byKey)) {
      w[k] = (
        <div>
          {ts.map((t) => (
            <ThreadView key={t.id} t={t} slug={slug} />
          ))}
        </div>
      );
    }
    if (draft) {
      w[draft.key] = (
        <div>
          {w[draft.key]}
          <NewThreadForm slug={slug} n={n} path={path} line={draft.line} side={draft.side} onDone={() => setDraft(null)} />
        </div>
      );
    }
    return w;
  }, [file, threads, draft, slug, n, path]);
  const count = threads.filter((t) => !t.resolved).length;
  const adds = file.hunks.reduce((a, h) => a + h.changes.filter(isInsert).length, 0);
  const dels = file.hunks.reduce((a, h) => a + h.changes.filter(isDelete).length, 0);
  return (
    <div className="overflow-hidden rounded-xl border border-line bg-surface shadow-card">
      <button className={clsx("flex w-full items-center gap-2 bg-surface-2/70 px-3 py-2.5 text-left text-sm", !collapsed && "border-b border-line")} onClick={() => setCollapsed(!collapsed)}>
        {collapsed ? <ChevronRight size={14} className="shrink-0 text-fg-subtle" /> : <ChevronDown size={14} className="shrink-0 text-fg-subtle" />}
        <FileCode size={14} className="shrink-0 text-fg-subtle" />
        <span className="min-w-0 flex-1 truncate font-mono text-xs">{file.type === "rename" ? `${file.oldPath} → ${file.newPath}` : path}</span>
        {count > 0 && (
          <Pill tone="amber" className="shrink-0">
            {count} open
          </Pill>
        )}
        <span className="shrink-0 font-mono text-[11px] tabular-nums">
          <span className="text-emerald-600 dark:text-emerald-400">+{adds}</span> <span className="text-rose-600 dark:text-rose-400">−{dels}</span>
        </span>
      </button>
      {!collapsed && (
        <Diff
          viewType="unified"
          diffType={file.type}
          hunks={file.hunks}
          widgets={widgets}
          gutterEvents={
            readOnly
              ? {}
              : {
                  onClick: ({ change }) => {
                    if (!change) return;
                    const line = isDelete(change) ? change.lineNumber : rightLine(change);
                    if (line == null) return;
                    setDraft({ key: getChangeKey(change), line, side: isDelete(change) ? "LEFT" : "RIGHT" });
                  },
                }
          }
        >
          {(hunks) => hunks.map((h) => <Hunk key={h.content} hunk={h} />)}
        </Diff>
      )}
    </div>
  );
}

function FilesTab({ pr, slug }: { pr: PullDetail; slug: string }) {
  const [sinceReview, setSinceReview] = useState(false);
  const since = sinceReview && pr.last_reviewed_sha && pr.last_reviewed_sha !== pr.head_sha ? pr.last_reviewed_sha : undefined;
  const diff = useQuery({
    queryKey: ["pull", "diff", slug, pr.number, pr.head_sha, since],
    queryFn: () => unwrap(client.GET("/api/projects/{p}/pulls/{n}/diff", { params: { path: { p: slug, n: pr.number }, query: { since } } })),
  });
  const files = useMemo(() => (diff.data ? diff.data.files.flatMap((f) => parseDiff(f.patch)) : []), [diff.data]);
  const outdated = pr.threads.filter((t) => t.outdated);
  const orphan = pr.threads.filter((t) => !t.outdated && !files.some((f) => (f.type === "delete" ? f.oldPath : f.newPath) === t.path));
  return (
    <div className="space-y-3">
      <div className="flex flex-wrap items-center gap-x-4 gap-y-2 text-sm text-fg-muted">
        <span>
          <b className="font-semibold text-fg">{files.length}</b> file{files.length === 1 ? "" : "s"} ·{" "}
          <code className="font-mono text-xs">{short(diff.data?.from)}</code>..<code className="font-mono text-xs">{short(diff.data?.head)}</code>
        </span>
        {pr.last_reviewed_sha && pr.last_reviewed_sha !== pr.head_sha && (
          <label className="flex items-center gap-2 text-xs">
            <Switch size="sm" checked={sinceReview} onChange={setSinceReview} label="Only changes since last review" />
            Only changes since last review ({short(pr.last_reviewed_sha)})
          </label>
        )}
        <span className="ml-auto hidden items-center gap-1.5 text-xs text-fg-subtle sm:flex">
          <MessageSquarePlus size={13} /> Click a line number to comment
        </span>
      </div>
      <ErrorBox error={diff.error} />
      {files.map((f) => {
        const p = f.type === "delete" ? f.oldPath : f.newPath;
        return <FileView key={p} file={f} slug={slug} n={pr.number} threads={pr.threads.filter((t) => t.path === p)} readOnly={pr.state !== "open" || !!since} />;
      })}
      {(outdated.length > 0 || orphan.length > 0) && (
        <div className="space-y-1 pt-2">
          <div className="text-xs font-semibold text-fg-muted">Outdated threads</div>
          {[...outdated, ...orphan].map((t) => (
            <ThreadView key={t.id} t={t} slug={slug} compact />
          ))}
        </div>
      )}
    </div>
  );
}

function MergeBox({ pr, slug }: { pr: PullDetail; slug: string }) {
  const qc = useQueryClient();
  const m = useQuery({
    queryKey: ["pull", "mergeability", slug, pr.number, pr.head_sha],
    queryFn: () => unwrap(client.GET("/api/projects/{p}/pulls/{n}/mergeability", { params: { path: { p: slug, n: pr.number } } })),
  });
  const project = useQuery({ queryKey: ["project", slug], queryFn: () => unwrap(client.GET("/api/projects/{p}", { params: { path: { p: slug } } })) });
  const [strategy, setStrategy] = useState<string>("");
  const [message, setMessage] = useState(pr.default_merge_message);
  const [force, setForce] = useState(false);
  const merge = useMutation({
    mutationFn: () => api("POST", `/api/projects/${slug}/pulls/${pr.number}/merge`, { strategy: strategy || null, message, force }),
    onSuccess: () => qc.invalidateQueries(),
  });
  const [verdictBody, setVerdictBody] = useState("");
  const review = useMutation({
    mutationFn: (verdict: string) => api("POST", `/api/projects/${slug}/pulls/${pr.number}/reviews`, { verdict, body: verdictBody, commit_sha: pr.head_sha }),
    onSuccess: () => {
      setVerdictBody("");
      qc.invalidateQueries();
    },
  });
  const close = useMutation({ mutationFn: () => api("POST", `/api/projects/${slug}/pulls/${pr.number}/close`), onSuccess: () => qc.invalidateQueries() });
  if (pr.state === "merged")
    return (
      <div className="flex items-start gap-3 rounded-2xl border border-violet-500/30 bg-violet-500/8 p-4 text-sm">
        <GitMerge size={18} className="mt-0.5 shrink-0 text-violet-500" />
        <div>
          <div className="font-semibold">Merged</div>
          <div className="text-fg-muted">
            As <code className="font-mono text-xs">{short(pr.merged_sha)}</code> ({pr.merge_strategy}) <TimeAgo iso={pr.merged_at} />
          </div>
        </div>
      </div>
    );
  if (pr.state === "closed")
    return (
      <div className="flex items-center gap-3 rounded-2xl border border-line bg-surface-2/60 p-4 text-sm text-fg-muted">
        <GitPullRequestClosed size={18} className="shrink-0" /> Closed without merging.
      </div>
    );
  const strat = strategy || project.data?.merge_strategy || "merge";
  const regex = project.data?.commit_msg_regex;
  const blankMessage = message.trim() === "";
  const effectiveMessage = blankMessage ? pr.default_merge_message : message;
  let msgOk = true;
  try {
    msgOk = !regex || strat === "rebase" || new RegExp(regex, "u").test(effectiveMessage.split("\n")[0]);
  } catch {
    /* server validates */
  }
  const ready = !!m.data?.mergeable;
  return (
    <div className="space-y-4">
      <div className={clsx("overflow-hidden rounded-2xl border shadow-card", ready ? "border-emerald-500/35" : "border-line")}>
        <div className={clsx("flex items-center gap-3 px-4 py-3", ready ? "bg-emerald-500/8" : "bg-surface-2/60")}>
          <div className={clsx("grid h-8 w-8 shrink-0 place-items-center rounded-full", ready ? "bg-emerald-500 text-white" : "bg-surface-3 text-fg-subtle")}>
            {ready ? <CheckCircle2 size={18} /> : <GitPullRequest size={16} />}
          </div>
          <div className="text-sm font-semibold">{ready ? "Ready to merge" : "Not ready to merge"}</div>
        </div>
        <div className="space-y-3 bg-surface p-4">
          {m.data && m.data.blockers.length > 0 && (
            <ul className="space-y-1 text-xs break-words text-fg-muted">
              {m.data.blockers.map((b) => (
                <li key={b} className="flex gap-2">
                  <CircleDot size={12} className="mt-0.5 shrink-0 text-amber-500" />
                  {b}
                </li>
              ))}
            </ul>
          )}
          <select className={clsx(inputCls, selectCls)} value={strat} onChange={(e) => setStrategy(e.target.value)} aria-label="Merge strategy">
            <option value="squash">Squash and merge</option>
            <option value="merge">Create a merge commit</option>
            <option value="rebase">Rebase and merge</option>
          </select>
          {strat !== "rebase" && (
            <textarea
              className={clsx(inputCls, "h-24 resize-y font-mono sm:text-xs", !msgOk && "border-rose-500/60 ring-3 ring-rose-500/15")}
              placeholder="Leave blank to generate a message from the title and linked issues"
              value={message}
              onChange={(e) => setMessage(e.target.value)}
            />
          )}
          {!msgOk && <div className="text-xs break-words text-rose-600 dark:text-rose-400">First line must match {regex}</div>}
          <label className="flex items-center gap-2 text-xs text-fg-muted">
            <input type="checkbox" className="h-3.5 w-3.5 accent-accent" checked={force} onChange={(e) => setForce(e.target.checked)} /> Allow merge without approval
          </label>
          <Button
            variant="success"
            className="w-full justify-center whitespace-normal"
            disabled={merge.isPending || m.data?.has_conflicts || (!m.data?.mergeable && !force) || !msgOk}
            onClick={() => merge.mutate()}
          >
            <GitMerge size={15} /> Merge into <span className="font-mono break-all">{pr.base_branch}</span>
          </Button>
          <ErrorBox error={merge.error ?? close.error} />
        </div>
      </div>
      <div className="space-y-3 rounded-2xl border border-line bg-surface p-4 shadow-card">
        <div className="text-sm font-semibold">
          Your review <span className="font-normal text-fg-subtle">at {short(pr.head_sha)}</span>
        </div>
        <textarea className={clsx(inputCls, "h-20 resize-y")} placeholder="Summary (markdown)" value={verdictBody} onChange={(e) => setVerdictBody(e.target.value)} />
        <div className="flex flex-wrap gap-2">
          <Button size="sm" variant="success" onClick={() => review.mutate("approve")}>
            Approve
          </Button>
          <Button size="sm" onClick={() => review.mutate("changes_requested")}>
            Request changes
          </Button>
          <Button size="sm" variant="ghost" disabled={!verdictBody.trim()} onClick={() => review.mutate("comment")}>
            Comment
          </Button>
        </div>
        <ErrorBox error={review.error} />
      </div>
      <button className="w-full text-center text-xs text-fg-subtle hover:text-rose-600 dark:hover:text-rose-400" onClick={() => confirm("Close this PR without merging?") && close.mutate()}>
        Close pull request without merging
      </button>
    </div>
  );
}

function Conversation({ pr, slug }: { pr: PullDetail; slug: string }) {
  const qc = useQueryClient();
  const [body, setBody] = useState("");
  const add = useMutation({
    mutationFn: () => api("POST", `/api/projects/${slug}/pulls/${pr.number}/comments`, { body }),
    onSuccess: () => {
      setBody("");
      qc.invalidateQueries({ queryKey: ["pull"] });
    },
  });
  const open = pr.threads.filter((t) => !t.resolved);
  return (
    <div className="space-y-5">
      <div className="flex items-start gap-3">
        <Avatar name={pr.author_name} agent={pr.author_kind === "agent"} size={28} />
        <div className="min-w-0 flex-1">
          <div className="mb-1.5 pt-1 text-xs text-fg-subtle">
            <b className="font-semibold text-fg">{pr.author_name}</b> opened <TimeAgo iso={pr.created_at} />
          </div>
          <div className="rounded-xl border border-line bg-surface px-3.5 py-3">
            {pr.body ? <Markdown>{pr.body}</Markdown> : <span className="text-sm text-fg-subtle italic">No description.</span>}
          </div>
        </div>
      </div>
      {pr.comments.map((c) => (
        <CommentView key={c.id} c={c} />
      ))}
      {open.length > 0 && (
        <div className="space-y-1">
          <div className="text-xs font-semibold text-fg-muted">{open.length} unresolved review threads</div>
          <div className="-m-2">
            {open.map((t) => (
              <ThreadView key={t.id} t={t} slug={slug} compact />
            ))}
          </div>
        </div>
      )}
      <div className="space-y-2">
        <textarea className={clsx(inputCls, "h-24 resize-y")} placeholder="Comment on the pull request…" value={body} onChange={(e) => setBody(e.target.value)} />
        <div className="flex justify-end">
          <Button variant="primary" disabled={!body.trim()} onClick={() => add.mutate()}>
            Comment
          </Button>
        </div>
      </div>
    </div>
  );
}

function Commits({ pr, slug }: { pr: PullDetail; slug: string }) {
  const q = useQuery({
    queryKey: ["pull", "commits", slug, pr.number, pr.head_sha],
    queryFn: () => unwrap(client.GET("/api/projects/{p}/pulls/{n}/commits", { params: { path: { p: slug, n: pr.number } } })),
  });
  return (
    <div className="divide-y divide-line overflow-hidden rounded-xl border border-line bg-surface shadow-card">
      {q.data?.map((c) => (
        <div key={c.sha} className="flex items-start gap-3 px-4 py-3 text-sm">
          <GitCommitHorizontal size={16} className="mt-0.5 shrink-0 text-fg-subtle" />
          <div className="min-w-0 flex-1">
            <div className="font-medium break-words">{c.subject}</div>
            <div className="text-xs text-fg-subtle">
              {c.author} · <TimeAgo iso={c.date} />
            </div>
          </div>
          <code className="shrink-0 rounded-md bg-surface-3 px-1.5 py-0.5 font-mono text-[11px] text-fg-muted">{short(c.sha)}</code>
        </div>
      ))}
    </div>
  );
}

const PR_STATE: Record<string, { tone: "green" | "violet" | "neutral"; icon: typeof GitPullRequest; label: string }> = {
  open: { tone: "green", icon: GitPullRequest, label: "Open" },
  merged: { tone: "violet", icon: GitMerge, label: "Merged" },
  closed: { tone: "neutral", icon: GitPullRequestClosed, label: "Closed" },
};

export default function PullPage() {
  const { slug = "", n = "" } = useParams();
  const [tab, setTab] = useState<"conversation" | "files" | "commits">("conversation");
  const pr = useQuery({
    queryKey: ["pull", slug, Number(n)],
    queryFn: () => unwrap(client.GET("/api/projects/{p}/pulls/{n}", { params: { path: { p: slug, n: Number(n) } } })),
  });
  if (pr.error)
    return (
      <Page>
        <ErrorBox error={pr.error} />
      </Page>
    );
  if (!pr.data) return null;
  const d = pr.data;
  const reviews = d.reviews.filter((r) => r.verdict !== "comment");
  const last = reviews[reviews.length - 1];
  const st = PR_STATE[d.state] ?? PR_STATE.closed;
  const openThreads = d.threads.filter((x) => !x.resolved).length;
  const chip = "inline-flex min-w-0 items-center rounded-md bg-surface-3 px-1.5 py-0.5 font-mono text-xs text-fg-muted";
  return (
    <Page width="xl">
      {d.issues.length > 0 && (
        <Link to={`/p/${slug}/issues/${d.issues[0]}`} className="mb-4 inline-flex items-center gap-1.5 text-sm text-fg-muted hover:text-fg">
          <ArrowLeft size={14} /> Issue #{d.issues[0]}
        </Link>
      )}
      <div className="mb-5 space-y-3">
        <h1 className="text-xl leading-tight font-semibold tracking-tight text-balance sm:text-2xl">
          {d.title} <span className="font-normal text-fg-subtle">#{d.number}</span>
        </h1>
        <div className="flex flex-wrap items-center gap-2 text-sm text-fg-muted">
          <Pill tone={st.tone} className="h-6 px-2.5 text-xs">
            <st.icon size={13} /> {st.label}
          </Pill>
          <span className="flex max-w-full min-w-0 items-center gap-1.5">
            <code className={clsx(chip, "block truncate")} title={d.branch}>
              {d.branch}
            </code>
            <ArrowRight size={13} className="shrink-0 text-fg-subtle" />
            <code className={clsx(chip, "shrink-0")}>{d.base_branch}</code>
          </span>
          <span className="text-xs text-fg-subtle">
            head <code className="font-mono">{short(d.head_sha)}</code>
          </span>
          {d.issues.map((i) => (
            <Link key={i} to={`/p/${slug}/issues/${i}`} className="text-xs font-medium text-accent-fg hover:underline">
              #{i}
            </Link>
          ))}
          {last && (
            <Pill tone={last.verdict === "approve" ? "green" : last.verdict === "changes_requested" ? "orange" : "amber"}>
              {last.verdict.replace("_", " ")} @ {short(last.commit_sha)}
            </Pill>
          )}
          {d.github_url && (
            <a href={d.github_url} target="_blank" className="inline-flex items-center gap-1 text-xs text-fg-subtle hover:text-fg">
              GitHub <ExternalLink size={11} />
            </a>
          )}
        </div>
      </div>
      <div className="grid grid-cols-1 gap-6 lg:grid-cols-[minmax(0,1fr)_340px] lg:gap-8">
        <div className="min-w-0">
          <Tabs
            className="mb-5"
            value={tab}
            onChange={setTab}
            tabs={[
              { id: "conversation", label: "Conversation", badge: <span className="text-xs text-fg-subtle tabular-nums">{d.comments.length}</span> },
              { id: "files", label: "Files", badge: openThreads > 0 ? <Pill tone="amber">{openThreads} open</Pill> : undefined },
              { id: "commits", label: "Commits" },
            ]}
          />
          {tab === "conversation" && <Conversation pr={d} slug={slug} />}
          {tab === "files" && <FilesTab pr={d} slug={slug} />}
          {tab === "commits" && <Commits pr={d} slug={slug} />}
        </div>
        <div className="min-w-0 max-lg:order-first lg:sticky lg:top-6 lg:self-start">
          <MergeBox key={d.head_sha ?? ""} pr={d} slug={slug} />
        </div>
      </div>
    </Page>
  );
}
