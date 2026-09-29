import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import clsx from "clsx";
import {
  ArrowRight,
  Bot,
  CheckCircle2,
  ChevronDown,
  CircleHelp,
  CircleSlash,
  GitBranch,
  GitMerge,
  GitPullRequest,
  Loader2,
  Pause,
  PauseCircle,
  Pencil,
  Play,
  RotateCcw,
  X,
  XCircle,
} from "lucide-react";
import { type ReactNode, useEffect, useState } from "react";
import { Link, useNavigate, useParams } from "react-router";
import { type Comment, type IssueDetail, type IssueState, ROLE_LABEL, type Role, STATE_LABEL, api, client, unwrap } from "../api/client";
import { ImageTextarea } from "../components/ImageTextarea";
import { useMe } from "../components/Layout";
import { Avatar, Button, ErrorBox, HoldBadge, LabelChip, LiveDot, Markdown, Pill, STATE_TONE, StateBadge, TONE_DOT, TimeAgo, fieldSmCls, inputCls, selectCls } from "../components/ui";

const COMMENT_STYLE: Partial<Record<Comment["kind"], string>> = {
  decision_request: "border-amber-500/35 bg-amber-500/8",
  decision: "border-emerald-500/35 bg-emerald-500/8",
  review_summary: "border-indigo-500/25 bg-indigo-500/6",
};

export function CommentView({ c }: { c: Comment }) {
  if (c.kind === "system") {
    return (
      <div className="flex items-start gap-3 text-xs text-fg-muted">
        <span className="grid w-7 shrink-0 place-items-center pt-1.5" aria-hidden>
          <span className="h-2 w-2 rounded-full bg-line-strong ring-4 ring-surface" />
        </span>
        <Markdown className="min-w-0 flex-1 text-xs leading-relaxed text-fg-muted">{c.body}</Markdown>
        <TimeAgo iso={c.created_at} className="shrink-0 pt-0.5 text-fg-subtle" />
      </div>
    );
  }
  return (
    <div className="flex items-start gap-3">
      <Avatar name={c.author_name} agent={c.author_kind === "agent"} size={28} className="ring-4 ring-surface" />
      <div className="min-w-0 flex-1">
        <div className="mb-1.5 flex flex-wrap items-center gap-x-2 gap-y-1 pt-1 text-xs">
          <span className="font-semibold text-fg">{c.author_name}</span>
          {c.kind === "decision_request" && <Pill tone="amber">Decision needed</Pill>}
          {c.kind === "decision" && <Pill tone="green">Decision</Pill>}
          {c.kind === "review_summary" && <Pill tone="indigo">Review</Pill>}
          <TimeAgo iso={c.created_at} className="text-fg-subtle" />
        </div>
        <div className={clsx("rounded-xl border px-3.5 py-3", COMMENT_STYLE[c.kind] ?? "border-line bg-surface")}>
          <Markdown>{c.body}</Markdown>
        </div>
      </div>
    </div>
  );
}

const RUN_ICON: Record<string, [typeof CheckCircle2, string]> = {
  succeeded: [CheckCircle2, "text-emerald-500"],
  failed: [XCircle, "text-rose-500"],
  rate_limited: [PauseCircle, "text-amber-500"],
  cancelled: [CircleSlash, "text-fg-subtle"],
  interrupted: [RotateCcw, "text-fg-subtle"],
};

function RunEntry({ r }: { r: IssueDetail["runs"][number] }) {
  const live = ["queued", "preparing", "running"].includes(r.status);
  const [Icon, color] = RUN_ICON[r.status] ?? [Loader2, "text-sky-500"];
  return (
    <Link to={`/runs/${r.id}`} className="group flex items-center gap-3 text-xs text-fg-muted">
      <span className="grid h-7 w-7 shrink-0 place-items-center rounded-full border border-line bg-surface ring-4 ring-surface">
        {live ? <Loader2 size={14} className="animate-spin text-sky-500" /> : <Icon size={14} className={color} />}
      </span>
      <span className="min-w-0 flex-1 truncate">
        <span className="font-medium text-fg group-hover:underline">
          {ROLE_LABEL[r.role]} run #{r.id}
        </span>{" "}
        {r.status.replace("_", " ")}
        {r.error ? <span className="text-fg-subtle"> — {r.error.slice(0, 140)}</span> : ""}
      </span>
      <TimeAgo iso={r.created_at} className="shrink-0 text-fg-subtle" />
    </Link>
  );
}

function DecisionPanel({ issue, slug }: { issue: IssueDetail; slug: string }) {
  const qc = useQueryClient();
  const [answer, setAnswer] = useState("");
  const [resume, setResume] = useState<string>("");
  const req = [...issue.comments].reverse().find((c) => c.kind === "decision_request");
  const m = useMutation({
    mutationFn: () => api("POST", `/api/projects/${slug}/issues/${issue.number}/decision`, { answer, resume_to: resume || null }),
    onSuccess: () => {
      setAnswer("");
      qc.invalidateQueries();
    },
  });
  return (
    <div className="space-y-3 rounded-2xl border border-amber-500/40 bg-amber-500/6 p-4">
      <div className="flex items-center gap-2 text-sm font-semibold text-amber-800 dark:text-amber-300">
        <CircleHelp size={16} /> A decision is needed
      </div>
      {req ? <Markdown>{req.body}</Markdown> : <div className="text-sm">{issue.hold_reason}</div>}
      <textarea className={clsx(inputCls, "h-24 resize-y")} placeholder="Your decision (agents will treat this as settled)…" value={answer} onChange={(e) => setAnswer(e.target.value)} />
      <div className="flex flex-col gap-2 sm:flex-row sm:items-center">
        <select className={clsx(inputCls, selectCls, "min-w-0 sm:flex-1")} value={resume} onChange={(e) => setResume(e.target.value)} aria-label="After deciding">
          <option value="">Stay in {STATE_LABEL[issue.state]}</option>
          {(["ready", "in_progress", "changes_requested", "in_review", "backlog"] as IssueState[])
            .filter((s) => s !== issue.state)
            .map((s) => (
              <option key={s} value={s}>
                Resume → {STATE_LABEL[s]}
              </option>
            ))}
        </select>
        <Button variant="primary" className="justify-center" disabled={!answer.trim() || m.isPending} onClick={() => m.mutate()}>
          Record decision
        </Button>
      </div>
      <ErrorBox error={m.error} />
    </div>
  );
}

function Prop({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="space-y-1.5">
      <div className="text-[11px] font-medium text-fg-subtle">{label}</div>
      {children}
    </div>
  );
}

function Meta({ issue, slug }: { issue: IssueDetail; slug: string }) {
  const qc = useQueryClient();
  const patch = useMutation({
    mutationFn: (body: Record<string, unknown>) => api("PATCH", `/api/projects/${slug}/issues/${issue.number}`, body),
    onSuccess: () => qc.invalidateQueries(),
  });
  const [labels, setLabels] = useState(issue.labels.map((l) => l.name).join(", "));
  useEffect(() => setLabels(issue.labels.map((l) => l.name).join(", ")), [issue.labels]);
  return (
    <>
      <div className="grid grid-cols-2 gap-3">
        <Prop label="Priority">
          <select className={clsx(fieldSmCls, selectCls, "w-full")} value={issue.priority ?? ""} onChange={(e) => patch.mutate({ priority: e.target.value })} aria-label="Priority">
            <option value="">None</option>
            <option>P0</option>
            <option>P1</option>
            <option>P2</option>
          </select>
        </Prop>
        <Prop label="Size">
          <select className={clsx(fieldSmCls, selectCls, "w-full")} value={issue.size ?? ""} onChange={(e) => patch.mutate({ size: e.target.value })} aria-label="Size">
            <option value="">None</option>
            {["XS", "S", "M", "L", "XL"].map((s) => (
              <option key={s}>{s}</option>
            ))}
          </select>
        </Prop>
      </div>
      <Prop label="Labels">
        {issue.labels.length > 0 && (
          <div className="flex flex-wrap gap-1">
            {issue.labels.map((l) => (
              <LabelChip key={l.name} name={l.name} color={l.color} />
            ))}
          </div>
        )}
        <input
          className={clsx(fieldSmCls, "w-full")}
          value={labels}
          aria-label="Labels"
          placeholder="Comma-separated"
          onChange={(e) => setLabels(e.target.value)}
          onBlur={() => {
            const next = labels.split(",").map((s) => s.trim()).filter(Boolean);
            if (next.join(",") !== issue.labels.map((l) => l.name).join(",")) patch.mutate({ labels: next });
          }}
        />
      </Prop>
      {issue.branch_name && (
        <Prop label="Branch">
          <div className="flex items-start gap-1.5 text-xs">
            <GitBranch size={13} className="mt-0.5 shrink-0 text-fg-subtle" />
            <code className="min-w-0 font-mono break-all text-fg-muted">{issue.branch_name}</code>
          </div>
        </Prop>
      )}
      {(issue.parent || issue.children.length > 0) && (
        <Prop label="Related">
          <div className="flex flex-wrap gap-1.5 text-xs">
            {issue.parent && (
              <Link className="rounded-md bg-surface-3 px-1.5 py-0.5 text-accent-fg hover:underline" to={`/p/${slug}/issues/${issue.parent}`}>
                parent #{issue.parent}
              </Link>
            )}
            {issue.children.map((c) => (
              <Link key={c} className="rounded-md bg-surface-3 px-1.5 py-0.5 text-accent-fg hover:underline" to={`/p/${slug}/issues/${c}`}>
                #{c}
              </Link>
            ))}
          </div>
        </Prop>
      )}
      <div className="space-y-1 border-t border-line pt-3 text-xs text-fg-muted">
        <div>
          Reported by <span className="text-fg">{issue.author_name ?? "?"}</span> ({issue.source}) <TimeAgo iso={issue.created_at} />
        </div>
        {issue.github_number && <div>GitHub #{issue.github_number}</div>}
        {issue.failure_count > 0 && <div className="text-rose-600 dark:text-rose-400">{issue.failure_count} failed run(s)</div>}
        {issue.next_attempt_at && (
          <div>
            Next attempt <TimeAgo iso={issue.next_attempt_at} />
          </div>
        )}
      </div>
      <ErrorBox error={patch.error} />
    </>
  );
}

function RunControls({ issue, slug }: { issue: IssueDetail; slug: string }) {
  const qc = useQueryClient();
  const agents = useQuery({ queryKey: ["agents"], queryFn: () => unwrap(client.GET("/api/agents")) });
  const [role, setRole] = useState<string>("");
  const [agent, setAgent] = useState<string>("");
  const start = useMutation({
    mutationFn: () => api("POST", `/api/projects/${slug}/issues/${issue.number}/runs`, { role: role || null, agent: agent || null }),
    onSuccess: () => qc.invalidateQueries(),
  });
  const active = issue.runs.find((r) => ["queued", "preparing", "running"].includes(r.status));
  if (active)
    return (
      <Link
        to={`/runs/${active.id}`}
        className="flex items-center gap-2 rounded-lg border border-sky-500/25 bg-sky-500/10 px-3 py-2 text-xs font-medium text-sky-700 transition-colors hover:bg-sky-500/15 dark:text-sky-300"
      >
        <LiveDot tone="sky" />
        <span className="min-w-0 flex-1 truncate">
          {ROLE_LABEL[active.role]} run #{active.id} {active.status}
        </span>
        <ArrowRight size={13} />
      </Link>
    );
  return (
    <div className="space-y-2">
      <div className="grid grid-cols-2 gap-2">
        <select className={clsx(fieldSmCls, selectCls, "w-full")} value={role} onChange={(e) => setRole(e.target.value)} aria-label="Role">
          <option value="">Auto role</option>
          {(["triage", "fix", "review", "merge_prep"] as Role[]).map((r) => (
            <option key={r} value={r}>
              {ROLE_LABEL[r]}
            </option>
          ))}
        </select>
        <select className={clsx(fieldSmCls, selectCls, "w-full")} value={agent} onChange={(e) => setAgent(e.target.value)} aria-label="Agent">
          <option value="">Default agent</option>
          {agents.data?.map((a) => (
            <option key={a.slug} value={a.slug}>
              {a.name}
            </option>
          ))}
        </select>
      </div>
      <Button
        variant="soft"
        className="w-full justify-center"
        onClick={() => start.mutate()}
        disabled={start.isPending || !!issue.hold}
        title={issue.hold ? "Clear the hold first" : "Start an agent run now"}
      >
        <Play size={13} /> Run agent now
      </Button>
      {start.error ? <div className="text-xs text-rose-600 dark:text-rose-400">{(start.error as Error).message}</div> : null}
    </div>
  );
}

function MoveTo({ issue, slug }: { issue: IssueDetail; slug: string }) {
  const qc = useQueryClient();
  const [closing, setClosing] = useState(false);
  const [reason, setReason] = useState("wontfix");
  const transition = useMutation({
    mutationFn: ({ to, reason }: { to: IssueState; reason?: string }) => api("POST", `/api/projects/${slug}/issues/${issue.number}/transition`, { to, close_reason: reason }),
    onSuccess: () => {
      setClosing(false);
      qc.invalidateQueries();
    },
  });
  const chip = "inline-flex h-7 items-center gap-1.5 rounded-lg border border-line bg-surface px-2 text-xs font-medium text-fg-muted shadow-card transition-colors hover:border-line-strong hover:text-fg";
  return (
    <div className="space-y-2">
      <div className="flex flex-wrap gap-1.5">
        {issue.allowed_transitions
          .filter((s) => s !== "closed")
          .map((s) => (
            <button key={s} onClick={() => transition.mutate({ to: s })} className={chip} disabled={transition.isPending}>
              <span className={clsx("h-1.5 w-1.5 rounded-full", TONE_DOT[STATE_TONE[s]])} />
              {STATE_LABEL[s]}
            </button>
          ))}
        {issue.allowed_transitions.includes("closed") && !closing && (
          <button onClick={() => setClosing(true)} className={chip}>
            <X size={12} /> Close…
          </button>
        )}
      </div>
      {closing && (
        <div className="flex gap-1.5">
          <select className={clsx(fieldSmCls, selectCls, "min-w-0 flex-1")} value={reason} onChange={(e) => setReason(e.target.value)} aria-label="Close reason">
            {["wontfix", "duplicate", "invalid", "not_planned"].map((r) => (
              <option key={r}>{r}</option>
            ))}
          </select>
          <Button size="sm" variant="danger" onClick={() => transition.mutate({ to: "closed", reason })}>
            Close
          </Button>
          <Button size="sm" variant="ghost" onClick={() => setClosing(false)} aria-label="Cancel closing">
            <X size={13} />
          </Button>
        </div>
      )}
      <ErrorBox error={transition.error} />
    </div>
  );
}

export function IssueBody({ issue, slug, showTitle }: { issue: IssueDetail; slug: string; showTitle?: boolean }) {
  const qc = useQueryClient();
  const me = useMe();
  const [editing, setEditing] = useState(false);
  const [title, setTitle] = useState(issue.title);
  const [body, setBody] = useState(issue.body);
  const [comment, setComment] = useState("");
  const [holdReason, setHoldReason] = useState("");
  const [descUploading, setDescUploading] = useState(false);
  const [commentUploading, setCommentUploading] = useState(false);
  const save = useMutation({
    mutationFn: () => api("PATCH", `/api/projects/${slug}/issues/${issue.number}`, { title, body }),
    onSuccess: () => {
      setEditing(false);
      qc.invalidateQueries();
    },
  });
  const addComment = useMutation({
    mutationFn: () => api("POST", `/api/projects/${slug}/issues/${issue.number}/comments`, { body: comment }),
    onSuccess: () => {
      setComment("");
      qc.invalidateQueries({ queryKey: ["issue"] });
    },
  });
  const hold = useMutation({
    mutationFn: (on: boolean) =>
      on
        ? api("PUT", `/api/projects/${slug}/issues/${issue.number}/hold`, { hold: "paused", reason: holdReason || "paused by a human" })
        : api("DELETE", `/api/projects/${slug}/issues/${issue.number}/hold`),
    onSuccess: () => qc.invalidateQueries(),
  });
  useEffect(() => {
    setTitle(issue.title);
    setBody(issue.body);
  }, [issue.title, issue.body]);

  const timeline = [
    ...issue.comments.map((c) => ({ t: c.created_at, el: <CommentView key={`c${c.id}`} c={c} /> })),
    ...issue.runs.map((r) => ({ t: r.created_at, el: <RunEntry key={`r${r.id}`} r={r} /> })),
  ].sort((a, b) => a.t.localeCompare(b.t));

  return (
    <div className="@container">
      <div className="grid grid-cols-1 gap-6 @3xl:grid-cols-[minmax(0,1fr)_17rem] @3xl:grid-rows-[auto_1fr] @3xl:gap-x-10">
        {/* Title, description and anything blocking */}
        <div className="min-w-0 space-y-5">
          {editing ? (
            <div className="space-y-3">
              {showTitle !== false && (
                <input
                  className="w-full rounded-lg border border-line bg-surface px-3 py-2 text-lg font-semibold tracking-tight shadow-card outline-none focus:border-accent/60 focus:ring-3 focus:ring-accent/15"
                  value={title}
                  onChange={(e) => setTitle(e.target.value)}
                  aria-label="Title"
                />
              )}
              <ImageTextarea slug={slug} className={clsx(inputCls, "h-72 resize-y font-mono sm:text-[13px]")} value={body} onChange={setBody} onPendingChange={setDescUploading} />
              <div className="flex gap-2">
                <Button variant="primary" disabled={descUploading || save.isPending} onClick={() => save.mutate()}>
                  Save
                </Button>
                <Button variant="ghost" onClick={() => setEditing(false)}>
                  Cancel
                </Button>
              </div>
              <ErrorBox error={save.error} />
            </div>
          ) : (
            <div className="group space-y-3">
              <div className="flex items-start gap-3">
                {showTitle && <h1 className="min-w-0 flex-1 text-xl leading-tight font-semibold tracking-tight text-balance sm:text-2xl">{issue.title}</h1>}
                <Button
                  variant="ghost"
                  size="sm"
                  className={clsx(!showTitle && "ml-auto", "-mr-1 opacity-70 group-hover:opacity-100")}
                  onClick={() => setEditing(true)}
                  title="Edit title and description"
                >
                  <Pencil size={13} /> Edit
                </Button>
              </div>
              {issue.body ? <Markdown className="text-[14.5px]">{issue.body}</Markdown> : <p className="text-sm text-fg-subtle italic">No description.</p>}
            </div>
          )}

          {issue.hold === "needs_decision" && <DecisionPanel issue={issue} slug={slug} />}
          {issue.hold && issue.hold !== "needs_decision" && (
            <div className="flex flex-wrap items-center gap-3 rounded-2xl border border-line bg-surface-2/60 p-3.5 text-sm">
              <HoldBadge hold={issue.hold} />
              <span className="min-w-0 flex-1 text-fg-muted">{issue.hold_reason}</span>
              <Button size="sm" onClick={() => hold.mutate(false)}>
                Clear hold
              </Button>
            </div>
          )}
        </div>

        {/* Properties and actions */}
        <aside className="min-w-0 space-y-5 self-start rounded-2xl border border-line bg-surface-2/50 p-4 @3xl:sticky @3xl:top-4 @3xl:row-span-2">
          <Prop label="Move to">
            <MoveTo issue={issue} slug={slug} />
          </Prop>
          <Prop label="Agent">
            <RunControls issue={issue} slug={slug} />
            {!issue.hold && (
              <div className="flex gap-1.5 pt-1">
                <input className={clsx(fieldSmCls, "min-w-0 flex-1")} placeholder="Pause reason (optional)" value={holdReason} onChange={(e) => setHoldReason(e.target.value)} aria-label="Pause reason" />
                <Button size="sm" onClick={() => hold.mutate(true)} title="Pause: agents won't pick this up">
                  <Pause size={12} /> Pause
                </Button>
              </div>
            )}
          </Prop>
          <Meta issue={issue} slug={slug} />
        </aside>

        {/* Pull requests, activity and the composer */}
        <div className="min-w-0 space-y-6">
          {issue.pull_requests.length > 0 && (
            <div className="space-y-2">
              <h2 className="text-xs font-semibold text-fg-muted">Pull requests</h2>
              {issue.pull_requests.map((pr) => (
                <Link
                  key={pr.number}
                  to={`/p/${slug}/pulls/${pr.number}`}
                  className="flex items-center gap-3 rounded-xl border border-line bg-surface px-3.5 py-2.5 text-sm shadow-card transition-colors hover:border-line-strong"
                >
                  {pr.state === "merged" ? (
                    <GitMerge size={16} className="shrink-0 text-violet-500" />
                  ) : (
                    <GitPullRequest size={16} className={clsx("shrink-0", pr.state === "open" ? "text-emerald-500" : "text-fg-subtle")} />
                  )}
                  <span className="min-w-0 flex-1 truncate font-medium">{pr.title}</span>
                  <span className="font-mono text-xs text-fg-subtle">#{pr.number}</span>
                  {pr.has_conflicts && <Pill tone="red">Conflicts</Pill>}
                  {pr.approved_sha && pr.approved_sha === pr.head_sha && <Pill tone="green">Approved</Pill>}
                  <span className="hidden text-xs text-fg-subtle capitalize sm:inline">{pr.state}</span>
                </Link>
              ))}
            </div>
          )}

          <div className="space-y-4">
            <h2 className="flex items-center gap-2 text-xs font-semibold text-fg-muted">
              Activity <span className="font-normal text-fg-subtle tabular-nums">{timeline.length}</span>
            </h2>
            <div className="relative space-y-5 before:absolute before:top-3 before:bottom-3 before:left-[13.5px] before:w-px before:bg-line [&>*]:relative">{timeline.map((t) => t.el)}</div>
          </div>

          <div className="flex items-start gap-3">
            <Avatar name={me.data?.name} size={28} className="mt-1" />
            <div className="min-w-0 flex-1 space-y-2">
              <ImageTextarea
                slug={slug}
                className={clsx(inputCls, "h-24 resize-y")}
                placeholder="Leave a comment (markdown; paste or drop images)…"
                value={comment}
                onChange={setComment}
                onPendingChange={setCommentUploading}
              />
              <div className="flex justify-end">
                <Button variant="primary" disabled={!comment.trim() || addComment.isPending || commentUploading} onClick={() => addComment.mutate()}>
                  Comment
                </Button>
              </div>
              <ErrorBox error={addComment.error} />
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}

export default function IssueDrawer() {
  const { slug = "", n = "" } = useParams();
  const nav = useNavigate();
  const issue = useQuery({
    queryKey: ["issue", slug, Number(n)],
    queryFn: () => unwrap(client.GET("/api/projects/{p}/issues/{n}", { params: { path: { p: slug, n: Number(n) } } })),
  });
  const close = () => nav(`/p/${slug}${window.location.search}`);
  useEffect(() => {
    const k = (e: KeyboardEvent) => e.key === "Escape" && close();
    window.addEventListener("keydown", k);
    return () => window.removeEventListener("keydown", k);
  });
  const d = issue.data;
  return (
    <div className="fixed inset-0 z-40 flex justify-end" onMouseDown={close}>
      <div className="absolute inset-0 animate-fade-in bg-black/30 backdrop-blur-[2px]" />
      <div
        role="dialog"
        aria-modal="true"
        aria-label={d ? `#${d.number} ${d.title}` : "Issue"}
        className="relative flex h-full w-full max-w-5xl animate-sheet-up flex-col bg-surface shadow-overlay sm:animate-slide-in sm:border-l sm:border-line"
        onMouseDown={(e) => e.stopPropagation()}
      >
        <header className="shrink-0 border-b border-line bg-surface/90 pt-[env(safe-area-inset-top)] backdrop-blur-xl">
          <div className="flex h-14 items-center gap-2 px-3 sm:px-6">
            <button onClick={close} className="grid h-9 w-9 shrink-0 place-items-center rounded-lg text-fg-muted hover:bg-surface-2 hover:text-fg sm:hidden" aria-label="Close">
              <ChevronDown size={20} />
            </button>
            {d && (
              <>
                <span className="font-mono text-sm text-fg-subtle">#{d.number}</span>
                <StateBadge state={d.state} always />
                {d.hold && <HoldBadge hold={d.hold} reason={d.hold_reason} />}
                <span className="hidden truncate text-xs text-fg-subtle sm:inline">
                  Updated <TimeAgo iso={d.updated_at} />
                </span>
              </>
            )}
            <button onClick={close} className="ml-auto hidden h-8 w-8 shrink-0 place-items-center rounded-lg text-fg-subtle hover:bg-surface-2 hover:text-fg sm:grid" aria-label="Close" title="Close (Esc)">
              <X size={18} />
            </button>
          </div>
        </header>
        <div className="min-h-0 flex-1 overflow-y-auto overscroll-contain">
          {issue.error ? (
            <div className="p-6">
              <ErrorBox error={issue.error} />
            </div>
          ) : null}
          {d ? (
            <div className="px-4 pt-5 pb-[max(2.5rem,env(safe-area-inset-bottom))] sm:px-8 sm:pt-7">
              <IssueBody issue={d} slug={slug} showTitle />
            </div>
          ) : (
            !issue.error && (
              <div className="grid h-40 place-items-center text-fg-subtle">
                <Bot size={20} className="animate-pulse" />
              </div>
            )
          )}
        </div>
      </div>
    </div>
  );
}
