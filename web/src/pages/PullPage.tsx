import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import clsx from "clsx";
import { Bot, CheckCircle2, ChevronDown, ChevronRight, FileCode, GitMerge, GitPullRequest, MessageSquarePlus } from "lucide-react";
import { type ReactNode, useMemo, useState } from "react";
import { Diff, Hunk, type ChangeData, type FileData, getChangeKey, isDelete, isInsert, isNormal, parseDiff } from "react-diff-view";
import "react-diff-view/style/index.css";
import { Link, useParams } from "react-router";
import { type PullDetail, type Thread, api, client, unwrap } from "../api/client";
import { Button, ErrorBox, Markdown, Pill, TimeAgo, fieldCls, inputCls, short } from "../components/ui";
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
    <div className={clsx("m-2 rounded-md border bg-white dark:bg-zinc-900 font-sans text-sm", t.resolved ? "border-zinc-200 dark:border-zinc-800" : t.severity === "blocking" ? "border-amber-300 dark:border-amber-800" : "border-zinc-300 dark:border-zinc-700")}>
      <button className="flex w-full items-center gap-2 px-3 py-1.5 text-xs text-zinc-500 text-left" onClick={() => setOpen(!open)}>
        {open ? <ChevronDown size={12} className="shrink-0" /> : <ChevronRight size={12} className="shrink-0" />}
        {compact && (
          <code className="min-w-0 shrink truncate text-zinc-700 dark:text-zinc-300">
            {t.path}:{t.line}
          </code>
        )}
        <Pill className={clsx("shrink-0", t.severity === "blocking" ? "bg-amber-100 text-amber-800 dark:bg-amber-950 dark:text-amber-300" : "bg-zinc-100 text-zinc-600 dark:bg-zinc-800")}>{t.severity}</Pill>
        {t.resolved && <Pill className="shrink-0 bg-emerald-100 text-emerald-800 dark:bg-emerald-950 dark:text-emerald-300">resolved</Pill>}
        {t.outdated && <Pill className="shrink-0 bg-zinc-200 text-zinc-600 dark:bg-zinc-800">outdated</Pill>}
        <span className="min-w-0 flex-1 truncate">{t.comments[0]?.body.slice(0, 90)}</span>
      </button>
      {open && (
        <div className="border-t border-zinc-100 dark:border-zinc-800 px-3 py-2 space-y-2">
          {t.outdated && t.diff_hunk && <pre className="text-[11px] bg-zinc-50 dark:bg-zinc-950 p-2 rounded overflow-x-auto">{t.diff_hunk}</pre>}
          {t.comments.map((c) => (
            <div key={c.id}>
              <div className="text-xs text-zinc-500 mb-0.5">
                <b className="text-zinc-700 dark:text-zinc-300">
                  {c.author_kind === "agent" && <Bot size={11} className="inline -mt-0.5" />} {c.author_name}
                </b>{" "}
                <TimeAgo iso={c.created_at} />
              </div>
              <Markdown>{c.body}</Markdown>
            </div>
          ))}
          <div className="flex gap-2">
            <input className={clsx(inputCls, "py-1 text-xs")} placeholder="Reply…" value={reply} onChange={(e) => setReply(e.target.value)} onKeyDown={(e) => e.key === "Enter" && reply.trim() && act.mutate("reply")} />
            <Button size="sm" disabled={!reply.trim()} onClick={() => act.mutate("reply")}>
              Reply
            </Button>
            <Button size="sm" onClick={() => act.mutate(t.resolved ? "unresolve" : "resolve")}>
              {t.resolved ? "Unresolve" : "Resolve"}
            </Button>
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
    <div className="m-2 rounded-md border border-blue-300 dark:border-blue-800 bg-white dark:bg-zinc-900 p-2 font-sans space-y-2">
      <textarea autoFocus className={clsx(inputCls, "h-20 text-sm")} placeholder={`Comment on ${side === "RIGHT" ? "" : "old "}line ${line}…`} value={body} onChange={(e) => setBody(e.target.value)} />
      <div className="flex items-center gap-2">
        <select className={clsx(fieldCls, " py-1 text-xs")} value={severity} onChange={(e) => setSeverity(e.target.value)}>
          <option value="blocking">blocking</option>
          <option value="nit">nit</option>
        </select>
        <Button size="sm" variant="primary" disabled={!body.trim()} onClick={() => m.mutate()}>
          Add comment
        </Button>
        <Button size="sm" variant="ghost" onClick={onDone}>
          Cancel
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
  return (
    <div className="rounded-md border border-zinc-200 dark:border-zinc-800 bg-white dark:bg-zinc-900 overflow-hidden">
      <button className="flex w-full items-center gap-2 border-b border-zinc-200 dark:border-zinc-800 bg-zinc-50 dark:bg-zinc-900 px-3 py-2 text-sm" onClick={() => setCollapsed(!collapsed)}>
        {collapsed ? <ChevronRight size={14} /> : <ChevronDown size={14} />}
        <FileCode size={14} className="text-zinc-500 shrink-0" />
        <span className="font-mono text-xs min-w-0 truncate">{file.type === "rename" ? `${file.oldPath} → ${file.newPath}` : path}</span>
        {count > 0 && <Pill className="bg-amber-100 text-amber-800 shrink-0">{count} open</Pill>}
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
      <div className="flex flex-wrap items-center gap-3 text-sm text-zinc-600 dark:text-zinc-400">
        <span>
          {files.length} files · <code>{short(diff.data?.from)}</code>..<code>{short(diff.data?.head)}</code>
        </span>
        {pr.last_reviewed_sha && pr.last_reviewed_sha !== pr.head_sha && (
          <label className="flex items-center gap-1">
            <input type="checkbox" checked={sinceReview} onChange={(e) => setSinceReview(e.target.checked)} />
            Only changes since last review ({short(pr.last_reviewed_sha)})
          </label>
        )}
        <span className="ml-auto text-xs flex items-center gap-1">
          <MessageSquarePlus size={12} /> click a line number to comment
        </span>
      </div>
      <ErrorBox error={diff.error} />
      {files.map((f) => {
        const p = f.type === "delete" ? f.oldPath : f.newPath;
        return <FileView key={p} file={f} slug={slug} n={pr.number} threads={pr.threads.filter((t) => t.path === p)} readOnly={pr.state !== "open" || !!since} />;
      })}
      {(outdated.length > 0 || orphan.length > 0) && (
        <div className="space-y-1">
          <div className="text-sm font-medium text-zinc-500">Outdated threads</div>
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
      <div className="rounded-lg border border-violet-300 bg-violet-50 dark:bg-violet-950/30 dark:border-violet-900 p-3 text-sm">
        <GitMerge size={14} className="inline text-violet-600" /> Merged as <code>{short(pr.merged_sha)}</code> ({pr.merge_strategy}) <TimeAgo iso={pr.merged_at} />
      </div>
    );
  if (pr.state === "closed") return <div className="rounded-lg border p-3 text-sm text-zinc-500">Closed without merging.</div>;
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
  return (
    <div className="space-y-3">
      <div className={clsx("rounded-lg border p-3 space-y-2", m.data?.mergeable ? "border-emerald-300 dark:border-emerald-800" : "border-zinc-300 dark:border-zinc-700")}>
        <div className="flex items-center gap-2 text-sm font-medium">
          {m.data?.mergeable ? <CheckCircle2 size={16} className="text-emerald-600" /> : <GitPullRequest size={16} className="text-zinc-500" />}
          {m.data?.mergeable ? "Ready to merge" : "Not ready to merge"}
        </div>
        {m.data && m.data.blockers.length > 0 && (
          <ul className="text-xs text-zinc-600 dark:text-zinc-400 list-disc pl-5 break-words">
            {m.data.blockers.map((b) => (
              <li key={b}>{b}</li>
            ))}
          </ul>
        )}
        <div className="flex flex-wrap items-center gap-2">
          <select className={clsx(fieldCls, "py-1 min-w-0")} value={strat} onChange={(e) => setStrategy(e.target.value)}>
            <option value="squash">Squash</option>
            <option value="merge">Merge commit</option>
            <option value="rebase">Rebase</option>
          </select>
          <label className="text-xs flex items-center gap-1 text-zinc-500">
            <input type="checkbox" checked={force} onChange={(e) => setForce(e.target.checked)} /> merge without approval
          </label>
        </div>
        {strat !== "rebase" && (
          <textarea
            className={clsx(inputCls, "font-mono text-xs h-20", !msgOk && "ring-2 ring-rose-400")}
            placeholder="Leave blank to generate a message from the title and linked issues"
            value={message}
            onChange={(e) => setMessage(e.target.value)}
          />
        )}
        {!msgOk && <div className="text-xs text-rose-600 break-words">First line must match {regex}</div>}
        <div className="flex flex-wrap gap-2">
          <Button variant="success" disabled={merge.isPending || m.data?.has_conflicts || (!m.data?.mergeable && !force) || !msgOk} onClick={() => merge.mutate()}>
            <GitMerge size={14} /> Merge into <span className="whitespace-normal break-all">{pr.base_branch}</span>
          </Button>
          <Button variant="ghost" onClick={() => confirm("Close this PR without merging?") && close.mutate()}>
            Close PR
          </Button>
        </div>
        <ErrorBox error={merge.error ?? close.error} />
      </div>
      <div className="rounded-lg border border-zinc-200 dark:border-zinc-800 p-3 space-y-2">
        <div className="text-sm font-medium">Your review at {short(pr.head_sha)}</div>
        <textarea className={clsx(inputCls, "h-16 text-sm")} placeholder="Summary (markdown)" value={verdictBody} onChange={(e) => setVerdictBody(e.target.value)} />
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
    <div className="space-y-3">
      <div className="rounded-md border border-zinc-200 dark:border-zinc-800 bg-white dark:bg-zinc-900 p-3">
        <div className="text-xs text-zinc-500 mb-1">
          {pr.author_kind === "agent" && <Bot size={11} className="inline" />} {pr.author_name} opened <TimeAgo iso={pr.created_at} />
        </div>
        {pr.body ? <Markdown>{pr.body}</Markdown> : <span className="text-sm italic text-zinc-500">No description.</span>}
      </div>
      {pr.comments.map((c) => (
        <CommentView key={c.id} c={c} />
      ))}
      {open.length > 0 && (
        <div className="space-y-1">
          <div className="text-sm font-medium text-zinc-500">{open.length} unresolved review threads</div>
          {open.map((t) => (
            <ThreadView key={t.id} t={t} slug={slug} compact />
          ))}
        </div>
      )}
      <textarea className={clsx(inputCls, "h-20")} placeholder="Comment on the PR…" value={body} onChange={(e) => setBody(e.target.value)} />
      <div className="flex justify-end">
        <Button variant="primary" disabled={!body.trim()} onClick={() => add.mutate()}>
          Comment
        </Button>
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
    <div className="divide-y divide-zinc-200 dark:divide-zinc-800 rounded-md border border-zinc-200 dark:border-zinc-800 bg-white dark:bg-zinc-900">
      {q.data?.map((c) => (
        <div key={c.sha} className="px-3 py-2 text-sm">
          <div className="flex gap-2">
            <span className="flex-1">{c.subject}</span>
            <code className="text-xs text-zinc-500">{short(c.sha)}</code>
          </div>
          <div className="text-xs text-zinc-500">
            {c.author} · <TimeAgo iso={c.date} />
          </div>
        </div>
      ))}
    </div>
  );
}

export default function PullPage() {
  const { slug = "", n = "" } = useParams();
  const [tab, setTab] = useState<"conversation" | "files" | "commits">("conversation");
  const pr = useQuery({
    queryKey: ["pull", slug, Number(n)],
    queryFn: () => unwrap(client.GET("/api/projects/{p}/pulls/{n}", { params: { path: { p: slug, n: Number(n) } } })),
  });
  if (pr.error) return <ErrorBox error={pr.error} />;
  if (!pr.data) return null;
  const d = pr.data;
  const reviews = d.reviews.filter((r) => r.verdict !== "comment");
  const last = reviews[reviews.length - 1];
  return (
    <div className="mx-auto max-w-6xl p-6 space-y-4">
      <div>
        <h1 className="text-xl font-semibold">
          {d.title} <span className="text-zinc-500 font-normal">#{d.number}</span>
        </h1>
        <div className="mt-1 flex flex-wrap items-center gap-2 text-sm text-zinc-600 dark:text-zinc-400">
          <Pill className={d.state === "open" ? "bg-emerald-100 text-emerald-800" : d.state === "merged" ? "bg-violet-100 text-violet-800" : "bg-zinc-200 text-zinc-700"}>{d.state}</Pill>
          <code className="text-xs break-all">{d.branch}</code> → <code className="text-xs break-all">{d.base_branch}</code>
          <span>
            head <code className="text-xs">{short(d.head_sha)}</code>
          </span>
          {d.issues.map((i) => (
            <Link key={i} to={`/p/${slug}/issues/${i}`} className="text-blue-600">
              #{i}
            </Link>
          ))}
          {last && (
            <Pill className={last.verdict === "approve" ? "bg-emerald-100 text-emerald-800" : last.verdict === "changes_requested" ? "bg-orange-100 text-orange-800" : "bg-amber-100 text-amber-800"}>
              last verdict: {last.verdict.replace("_", " ")} @ {short(last.commit_sha)}
            </Pill>
          )}
          {d.github_url && (
            <a href={d.github_url} target="_blank" className="text-xs text-zinc-500 underline">
              GitHub
            </a>
          )}
        </div>
      </div>
      <div className="flex gap-4 border-b border-zinc-200 dark:border-zinc-800 text-sm">
        {(["conversation", "files", "commits"] as const).map((t) => (
          <button key={t} onClick={() => setTab(t)} className={clsx("pb-2 capitalize", tab === t ? "border-b-2 border-blue-600 font-medium" : "text-zinc-500")}>
            {t}
            {t === "conversation" && ` (${d.comments.length})`}
            {t === "files" && d.threads.some((x) => !x.resolved) && ` · ${d.threads.filter((x) => !x.resolved).length} open`}
          </button>
        ))}
      </div>
      <div className="grid grid-cols-1 md:grid-cols-[minmax(0,1fr)_320px] gap-6">
        <div className="min-w-0">
          {tab === "conversation" && <Conversation pr={d} slug={slug} />}
          {tab === "files" && <FilesTab pr={d} slug={slug} />}
          {tab === "commits" && <Commits pr={d} slug={slug} />}
        </div>
        <div className="min-w-0">
          <MergeBox key={d.head_sha ?? ""} pr={d} slug={slug} />
        </div>
      </div>
    </div>
  );
}
