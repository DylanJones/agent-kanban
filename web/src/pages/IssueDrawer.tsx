import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import clsx from "clsx";
import { Bot, GitMerge, GitPullRequest, Pause, Pencil, Play, X } from "lucide-react";
import { useEffect, useState } from "react";
import { Link, useNavigate, useParams } from "react-router";
import { type Comment, type IssueDetail, type IssueState, ROLE_LABEL, type Role, STATE_LABEL, api, client, unwrap } from "../api/client";
import { ImageTextarea } from "../components/ImageTextarea";
import { Button, ErrorBox, HoldBadge, LabelChip, Markdown, Pill, StateBadge, TimeAgo, fieldCls, inputCls } from "../components/ui";

export function CommentView({ c }: { c: Comment }) {
  const style =
    c.kind === "decision_request"
      ? "border-amber-300 bg-amber-50 dark:bg-amber-950/30 dark:border-amber-800"
      : c.kind === "decision"
        ? "border-emerald-300 bg-emerald-50 dark:bg-emerald-950/30 dark:border-emerald-800"
        : c.kind === "review_summary"
          ? "border-indigo-200 bg-indigo-50/60 dark:bg-indigo-950/30 dark:border-indigo-900"
          : c.kind === "system"
            ? "border-transparent bg-transparent"
            : "border-zinc-200 dark:border-zinc-800 bg-white dark:bg-zinc-900";
  if (c.kind === "system") {
    return (
      <div className="text-xs text-zinc-500 flex gap-2 px-1">
        <Markdown className="flex-1 text-xs">{c.body}</Markdown>
        <TimeAgo iso={c.created_at} />
      </div>
    );
  }
  return (
    <div className={clsx("rounded-md border p-3", style)}>
      <div className="mb-1 flex items-center gap-2 text-xs text-zinc-500">
        <span className="font-medium text-zinc-700 dark:text-zinc-300">
          {c.author_kind === "agent" && <Bot size={12} className="inline -mt-0.5 mr-0.5" />}
          {c.author_name}
        </span>
        {c.kind === "decision_request" && <Pill className="bg-amber-200 text-amber-900">decision needed</Pill>}
        {c.kind === "decision" && <Pill className="bg-emerald-200 text-emerald-900">decision</Pill>}
        <span className="ml-auto">
          <TimeAgo iso={c.created_at} />
        </span>
      </div>
      <Markdown>{c.body}</Markdown>
    </div>
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
    <div className="rounded-lg border-2 border-amber-300 dark:border-amber-800 bg-amber-50 dark:bg-amber-950/20 p-3 space-y-2">
      <div className="text-sm font-semibold">❓ A decision is needed</div>
      {req ? <Markdown>{req.body}</Markdown> : <div className="text-sm">{issue.hold_reason}</div>}
      <textarea className={clsx(inputCls, "h-24")} placeholder="Your decision (agents will treat this as settled)…" value={answer} onChange={(e) => setAnswer(e.target.value)} />
      <div className="flex items-center gap-2">
        <select className={clsx(fieldCls, "")} value={resume} onChange={(e) => setResume(e.target.value)}>
          <option value="">Stay in {STATE_LABEL[issue.state]}</option>
          {(["ready", "in_progress", "changes_requested", "in_review", "backlog"] as IssueState[])
            .filter((s) => s !== issue.state)
            .map((s) => (
              <option key={s} value={s}>
                Resume → {STATE_LABEL[s]}
              </option>
            ))}
        </select>
        <Button variant="primary" disabled={!answer.trim() || m.isPending} onClick={() => m.mutate()}>
          Record decision
        </Button>
      </div>
      <ErrorBox error={m.error} />
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
    <div className="space-y-3 text-sm">
      <div>
        <div className="text-xs text-zinc-500 mb-1">Priority / size</div>
        <div className="flex gap-2">
          <select className={clsx(inputCls, "py-1")} value={issue.priority ?? ""} onChange={(e) => patch.mutate({ priority: e.target.value })}>
            <option value="">—</option>
            <option>P0</option>
            <option>P1</option>
            <option>P2</option>
          </select>
          <select className={clsx(inputCls, "py-1")} value={issue.size ?? ""} onChange={(e) => patch.mutate({ size: e.target.value })}>
            <option value="">—</option>
            {["XS", "S", "M", "L", "XL"].map((s) => (
              <option key={s}>{s}</option>
            ))}
          </select>
        </div>
      </div>
      <div>
        <div className="text-xs text-zinc-500 mb-1">Labels</div>
        <div className="flex flex-wrap gap-1 mb-1">
          {issue.labels.map((l) => (
            <LabelChip key={l.name} name={l.name} color={l.color} />
          ))}
        </div>
        <input
          className={clsx(inputCls, "py-1 text-xs")}
          value={labels}
          onChange={(e) => setLabels(e.target.value)}
          onBlur={() => {
            const next = labels.split(",").map((s) => s.trim()).filter(Boolean);
            if (next.join(",") !== issue.labels.map((l) => l.name).join(",")) patch.mutate({ labels: next });
          }}
        />
      </div>
      {issue.branch_name && (
        <div>
          <div className="text-xs text-zinc-500">Branch</div>
          <code className="text-xs break-all">{issue.branch_name}</code>
        </div>
      )}
      {(issue.parent || issue.children.length > 0) && (
        <div>
          <div className="text-xs text-zinc-500">Related</div>
          {issue.parent && (
            <Link className="text-blue-600 text-xs" to={`/p/${slug}/issues/${issue.parent}`}>
              parent #{issue.parent}
            </Link>
          )}
          {issue.children.map((c) => (
            <Link key={c} className="text-blue-600 text-xs ml-2" to={`/p/${slug}/issues/${c}`}>
              #{c}
            </Link>
          ))}
        </div>
      )}
      <div className="text-xs text-zinc-500 space-y-0.5">
        <div>
          Reported by {issue.author_name ?? "?"} ({issue.source}) <TimeAgo iso={issue.created_at} />
        </div>
        {issue.github_number && <div>GitHub #{issue.github_number}</div>}
        {issue.failure_count > 0 && <div className="text-rose-600">{issue.failure_count} failed run(s)</div>}
        {issue.next_attempt_at && (
          <div>
            Next attempt <TimeAgo iso={issue.next_attempt_at} />
          </div>
        )}
      </div>
      <ErrorBox error={patch.error} />
    </div>
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
      <Link to={`/runs/${active.id}`} className="inline-flex items-center gap-1.5 rounded-md bg-blue-50 dark:bg-blue-950/40 px-2 py-1 text-xs text-blue-700 dark:text-blue-300">
        <span className="h-2 w-2 rounded-full bg-blue-500 animate-pulse" /> {ROLE_LABEL[active.role]} run #{active.id} {active.status} →
      </Link>
    );
  return (
    <div className="grid grid-cols-2 gap-1">
      <select className={clsx(fieldCls, "py-1 text-xs")} value={role} onChange={(e) => setRole(e.target.value)}>
        <option value="">auto role</option>
        {(["triage", "fix", "review", "merge_prep"] as Role[]).map((r) => (
          <option key={r} value={r}>
            {ROLE_LABEL[r]}
          </option>
        ))}
      </select>
      <select className={clsx(fieldCls, "py-1 text-xs")} value={agent} onChange={(e) => setAgent(e.target.value)}>
        <option value="">default agent</option>
        {agents.data?.map((a) => (
          <option key={a.slug} value={a.slug}>
            {a.name}
          </option>
        ))}
      </select>
      <Button size="sm" className="col-span-2 justify-center" onClick={() => start.mutate()} disabled={start.isPending || !!issue.hold} title={issue.hold ? "Clear the hold first" : "Start an agent run now"}>
        <Play size={11} /> Run
      </Button>
      {start.error ? <span className="col-span-2 text-xs text-rose-600">{(start.error as Error).message}</span> : null}
    </div>
  );
}

export function IssueBody({ issue, slug }: { issue: IssueDetail; slug: string }) {
  const qc = useQueryClient();
  const [editing, setEditing] = useState(false);
  const [title, setTitle] = useState(issue.title);
  const [body, setBody] = useState(issue.body);
  const [comment, setComment] = useState("");
  const [holdReason, setHoldReason] = useState("");
  const [descUploading, setDescUploading] = useState(false);
  const [commentUploading, setCommentUploading] = useState(false);
  const transition = useMutation({
    mutationFn: ({ to, reason }: { to: IssueState; reason?: string }) =>
      api("POST", `/api/projects/${slug}/issues/${issue.number}/transition`, { to, close_reason: reason }),
    onSuccess: () => qc.invalidateQueries(),
  });
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
    ...issue.runs.map((r) => ({
      t: r.created_at,
      el: (
        <Link key={`r${r.id}`} to={`/runs/${r.id}`} className="flex items-center gap-2 text-xs text-zinc-500 hover:text-zinc-800 dark:hover:text-zinc-200 px-1">
          <Bot size={12} />
          <span>
            {ROLE_LABEL[r.role]} run #{r.id} — <b>{r.status}</b>
            {r.error ? `: ${r.error.slice(0, 140)}` : ""}
          </span>
          <span className="ml-auto">
            <TimeAgo iso={r.created_at} />
          </span>
        </Link>
      ),
    })),
  ].sort((a, b) => a.t.localeCompare(b.t));

  return (
    <div className="grid grid-cols-[1fr_220px] gap-6">
      <div className="space-y-4 min-w-0">
        {editing ? (
          <div className="space-y-2">
            <input className={clsx(inputCls, "text-lg font-semibold")} value={title} onChange={(e) => setTitle(e.target.value)} />
            <ImageTextarea slug={slug} className={clsx(inputCls, "font-mono h-64 text-xs")} value={body} onChange={setBody} onPendingChange={setDescUploading} />
            <div className="flex gap-2">
              <Button variant="primary" disabled={descUploading} onClick={() => save.mutate()}>
                Save
              </Button>
              <Button onClick={() => setEditing(false)}>Cancel</Button>
            </div>
          </div>
        ) : (
          <div className="group relative rounded-md border border-zinc-200 dark:border-zinc-800 bg-white dark:bg-zinc-900 p-3">
            <button className="absolute right-2 top-2 opacity-0 group-hover:opacity-100 text-zinc-400" onClick={() => setEditing(true)} title="Edit">
              <Pencil size={13} />
            </button>
            {issue.body ? <Markdown>{issue.body}</Markdown> : <span className="text-sm italic text-zinc-500">No description.</span>}
          </div>
        )}

        {issue.hold === "needs_decision" && <DecisionPanel issue={issue} slug={slug} />}
        {issue.hold && issue.hold !== "needs_decision" && (
          <div className="flex items-center gap-2 rounded-md border border-zinc-300 dark:border-zinc-700 p-2 text-sm">
            <HoldBadge hold={issue.hold} /> <span className="flex-1 text-zinc-600 dark:text-zinc-400">{issue.hold_reason}</span>
            <Button size="sm" onClick={() => hold.mutate(false)}>
              Clear hold
            </Button>
          </div>
        )}

        {issue.pull_requests.length > 0 && (
          <div className="space-y-1">
            {issue.pull_requests.map((pr) => (
              <Link
                key={pr.number}
                to={`/p/${slug}/pulls/${pr.number}`}
                className="flex items-center gap-2 rounded-md border border-zinc-200 dark:border-zinc-800 bg-white dark:bg-zinc-900 px-3 py-2 text-sm hover:border-zinc-400"
              >
                {pr.state === "merged" ? <GitMerge size={14} className="text-violet-600" /> : <GitPullRequest size={14} className={pr.state === "open" ? "text-emerald-600" : "text-zinc-400"} />}
                <span className="font-mono text-xs text-zinc-500">#{pr.number}</span>
                <span className="flex-1 truncate">{pr.title}</span>
                {pr.has_conflicts && <Pill className="bg-red-100 text-red-800">conflicts</Pill>}
                {pr.approved_sha && pr.approved_sha === pr.head_sha && <Pill className="bg-emerald-100 text-emerald-800">approved</Pill>}
                <span className="text-xs text-zinc-500">{pr.state}</span>
              </Link>
            ))}
          </div>
        )}

        <div className="space-y-2">{timeline.map((t) => t.el)}</div>

        <div className="space-y-2">
          <ImageTextarea
            slug={slug}
            className={clsx(inputCls, "h-20")}
            placeholder="Comment (markdown)…"
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
      <div className="space-y-4">
        <div className="space-y-2">
          <div className="text-xs text-zinc-500">Move to</div>
          <div className="flex flex-wrap gap-1">
            {issue.allowed_transitions
              .filter((s) => s !== "closed")
              .map((s) => (
                <button
                  key={s}
                  onClick={() => transition.mutate({ to: s })}
                  className="rounded border border-zinc-300 dark:border-zinc-700 px-1.5 py-0.5 text-xs hover:bg-zinc-100 dark:hover:bg-zinc-800"
                >
                  {STATE_LABEL[s]}
                </button>
              ))}
            {issue.allowed_transitions.includes("closed") && (
              <button
                onClick={() => {
                  const r = prompt("Close reason (wontfix, duplicate, invalid, not_planned):", "wontfix");
                  if (r) transition.mutate({ to: "closed", reason: r });
                }}
                className="rounded border border-zinc-300 dark:border-zinc-700 px-1.5 py-0.5 text-xs hover:bg-zinc-100 dark:hover:bg-zinc-800"
              >
                Close…
              </button>
            )}
          </div>
          <ErrorBox error={transition.error} />
        </div>
        <div className="space-y-1">
          <div className="text-xs text-zinc-500">Agent</div>
          <RunControls issue={issue} slug={slug} />
          {!issue.hold && (
            <div className="flex gap-1 pt-1">
              <input className={clsx(inputCls, "py-1 text-xs")} placeholder="pause reason" value={holdReason} onChange={(e) => setHoldReason(e.target.value)} />
              <Button size="sm" onClick={() => hold.mutate(true)} title="Pause: agents won't pick this up">
                <Pause size={11} />
              </Button>
            </div>
          )}
        </div>
        <Meta issue={issue} slug={slug} />
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
  return (
    <div className="fixed inset-0 z-40 flex justify-end bg-black/20" onMouseDown={close}>
      <div className="h-full w-full max-w-4xl overflow-y-auto bg-zinc-50 dark:bg-zinc-950 shadow-2xl border-l border-zinc-200 dark:border-zinc-800" onMouseDown={(e) => e.stopPropagation()}>
        {issue.error ? <ErrorBox error={issue.error} /> : null}
        {issue.data && (
          <div className="p-5 space-y-4">
            <div className="flex items-start gap-3">
              <div className="flex-1">
                <h1 className="text-lg font-semibold leading-snug">
                  <span className="text-zinc-500 font-mono mr-2">#{issue.data.number}</span>
                  {issue.data.title}
                </h1>
                <div className="mt-1 flex items-center gap-1.5">
                  <StateBadge state={issue.data.state} always />
                  {issue.data.hold && <HoldBadge hold={issue.data.hold} reason={issue.data.hold_reason} />}
                  <span className="text-xs text-zinc-500">
                    updated <TimeAgo iso={issue.data.updated_at} />
                  </span>
                </div>
              </div>
              <button onClick={close} className="text-zinc-500 hover:text-zinc-900 dark:hover:text-zinc-100">
                <X size={18} />
              </button>
            </div>
            <IssueBody issue={issue.data} slug={slug} />
          </div>
        )}
      </div>
    </div>
  );
}
