import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import clsx from "clsx";
import { ArrowLeft, ArrowUp, Box, CheckCircle2, CircleSlash, Clock, Cpu, Folder, GitPullRequest, Loader2, PauseCircle, RotateCcw, Square, Terminal, XCircle } from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";
import { Link, useParams } from "react-router";
import { ROLE_LABEL, type RunEvent, type RunView, api, client, unwrap } from "../api/client";
import { RunUsageChips, TOTAL_TOKENS_HELP } from "../components/RunUsage";
import { summarizeSettings } from "../components/SessionSettings";
import { Transcript } from "../components/Transcript";
import { Button, EmptyState, ErrorBox, LiveDot, Page, PageHeader, Pill, Segmented, Switch, TimeAgo, type Tone, fmtDur } from "../components/ui";
import { fmtTokens } from "./Usage";

const RUN_STATUS_TONE: Record<string, Tone> = {
  queued: "neutral",
  preparing: "sky",
  running: "sky",
  succeeded: "green",
  failed: "red",
  cancelled: "neutral",
  rate_limited: "amber",
  interrupted: "neutral",
};

const STATUS_ICON: Record<string, [typeof CheckCircle2, string]> = {
  succeeded: [CheckCircle2, "text-emerald-500"],
  failed: [XCircle, "text-rose-500"],
  rate_limited: [PauseCircle, "text-amber-500"],
  cancelled: [CircleSlash, "text-fg-subtle"],
  interrupted: [RotateCcw, "text-fg-subtle"],
  queued: [Clock, "text-fg-subtle"],
};

function isLive(status: string) {
  return ["preparing", "running"].includes(status);
}

function RunStatusIcon({ status, size = 32 }: { status: string; size?: number }) {
  const [Icon, color] = STATUS_ICON[status] ?? [Loader2, "text-sky-500"];
  return (
    <span className={clsx("grid shrink-0 place-items-center rounded-full border", isLive(status) ? "border-sky-500/30 bg-sky-500/10" : "border-line bg-surface-2")} style={{ width: size, height: size }}>
      {isLive(status) ? <Loader2 size={size * 0.5} className="animate-spin text-sky-500" /> : <Icon size={size * 0.5} className={color} />}
    </span>
  );
}

function duration(r: RunView) {
  if (!r.started_at) return "";
  const end = r.ended_at ? new Date(r.ended_at).getTime() : Date.now();
  return fmtDur((end - new Date(r.started_at).getTime()) / 1000);
}

const FILTERS = [
  { value: "", label: "All" },
  { value: "active", label: "Active" },
  { value: "failed", label: "Failed" },
  { value: "rate_limited", label: "Rate limited" },
  { value: "succeeded", label: "Succeeded" },
];

export function RunsPage() {
  const [status, setStatus] = useState("");
  const runs = useQuery({
    queryKey: ["runs", "list", status],
    queryFn: () => unwrap(client.GET("/api/runs", { params: { query: { status: status || undefined, limit: 200 } } })),
    refetchInterval: 10000,
  });
  return (
    <Page width="xl">
      <PageHeader title="Runs" subtitle="Every agent session across your projects, newest first." actions={<Segmented label="Status" value={status} onChange={setStatus} options={FILTERS} />} />
      <ErrorBox error={runs.error} />
      <div className="overflow-hidden rounded-xl border border-line bg-surface shadow-card">
        {runs.data?.length === 0 && (
          <EmptyState icon={<Terminal size={20} />} title="No runs yet">
            Turn on the scheduler or start a run from an issue.
          </EmptyState>
        )}
        <div className="divide-y divide-line">
          {runs.data?.map((r) => (
            <Link key={r.id} to={`/runs/${r.id}`} className="group flex items-center gap-3 px-4 py-3 transition-colors hover:bg-surface-2/60 sm:gap-4">
              <RunStatusIcon status={r.status} />
              <div className="min-w-0 flex-1">
                <div className="flex items-center gap-2">
                  <span className="truncate text-sm font-medium">{r.issue_title ?? `Issue #${r.issue}`}</span>
                  {r.status !== "succeeded" && (
                    <Pill tone={RUN_STATUS_TONE[r.status]} className="shrink-0">
                      {isLive(r.status) && <LiveDot tone="sky" className="scale-75" />}
                      {r.status.replace("_", " ")}
                    </Pill>
                  )}
                </div>
                <div className="mt-0.5 flex min-w-0 items-center gap-1.5 text-xs text-fg-subtle">
                  <span className="shrink-0 font-mono">#{r.id}</span>
                  <span>·</span>
                  <span className="shrink-0 font-medium text-fg-muted">{ROLE_LABEL[r.role]}</span>
                  <span>·</span>
                  <span className="shrink-0">{r.agent}</span>
                  <span className="max-sm:hidden">·</span>
                  <span className="shrink-0 max-sm:hidden">
                    {r.project} #{r.issue}
                  </span>
                  {(r.error ?? r.outcome) && <span className="min-w-0 truncate max-md:hidden">— {r.error ?? r.outcome}</span>}
                </div>
              </div>
              <div className="flex shrink-0 flex-col items-end gap-0.5 text-xs text-fg-subtle tabular-nums">
                <TimeAgo iso={r.created_at} />
                <span className="max-sm:hidden" title={r.total_tokens > 0 ? TOTAL_TOKENS_HELP : undefined}>
                  {[r.total_tokens > 0 ? `${fmtTokens(r.total_tokens)} tok total` : "", duration(r)].filter(Boolean).join(" · ")}
                </span>
              </div>
            </Link>
          ))}
        </div>
      </div>
    </Page>
  );
}

type Payload = Record<string, unknown>;

export function useRunTranscript(id: number) {
  const [events, setEvents] = useState<Map<number, RunEvent>>(new Map());
  useEffect(() => {
    setEvents(new Map());
    const es = new EventSource(`/api/runs/${id}/stream`, { withCredentials: true });
    es.addEventListener("run_event", (e) => {
      const ev: RunEvent = JSON.parse((e as MessageEvent).data);
      setEvents((m) => new Map(m).set(ev.seq, ev));
    });
    es.addEventListener("end", () => es.close());
    return () => es.close();
  }, [id]);
  return useMemo(() => [...events.values()].sort((a, b) => a.seq - b.seq), [events]);
}

function MetaChip({ icon: Icon, children, title, className }: { icon: typeof Box; children: React.ReactNode; title?: string; className?: string }) {
  return (
    <span title={title} className={clsx("inline-flex min-w-0 items-center gap-1.5 rounded-md bg-surface-3/70 px-2 py-0.5 text-[11px] text-fg-muted", className)}>
      <Icon size={12} className="shrink-0 text-fg-subtle" />
      <span className="truncate">{children}</span>
    </span>
  );
}

export function RunDetail() {
  const { id = "" } = useParams();
  const qc = useQueryClient();
  const run = useQuery({ queryKey: ["run", Number(id)], queryFn: () => unwrap(client.GET("/api/runs/{id}", { params: { path: { id: Number(id) } } })), refetchInterval: 5000 });
  const events = useRunTranscript(Number(id));
  const [msg, setMsg] = useState("");
  const [follow, setFollow] = useState(true);
  const bottom = useRef<HTMLDivElement>(null);
  const cancel = useMutation({ mutationFn: () => api("POST", `/api/runs/${id}/cancel`), onSuccess: () => qc.invalidateQueries() });
  const send = useMutation({ mutationFn: () => api("POST", `/api/runs/${id}/messages`, { text: msg }), onSuccess: () => setMsg("") });
  // Follow the transcript only while you're at the bottom; scrolling up pauses it and
  // scrolling back down resumes it. The page scrolls inside <main>, not the window.
  useEffect(() => {
    const main = bottom.current?.closest("main");
    if (!main) return;
    const onScroll = () => setFollow(main.scrollHeight - main.scrollTop - main.clientHeight < 120);
    main.addEventListener("scroll", onScroll, { passive: true });
    return () => main.removeEventListener("scroll", onScroll);
  }, [run.data?.id]);
  useEffect(() => {
    const main = bottom.current?.closest("main");
    if (follow && main) main.scrollTop = main.scrollHeight;
  }, [events, follow]);
  const usage = [...events].reverse().find((e) => e.kind === "usage")?.payload as Payload | undefined;
  const r = run.data;
  const agents = useQuery({ queryKey: ["agents"], queryFn: () => unwrap(client.GET("/api/agents")) });
  const settings = r ? summarizeSettings(agents.data?.find((a) => a.slug === r.agent), r.session_config as Record<string, unknown> | null) : "";
  return (
    <div className="flex min-h-full flex-col">
      {r && (
        <header className="sticky top-0 z-10 border-b border-line bg-surface/85 backdrop-blur-xl">
          <div className="mx-auto w-full max-w-4xl space-y-2 px-4 py-3 sm:px-6">
            <div className="flex items-center gap-3">
              <Link to="/runs" className="-ml-1.5 grid h-8 w-8 shrink-0 place-items-center rounded-lg text-fg-muted hover:bg-surface-2 hover:text-fg" aria-label="All runs">
                <ArrowLeft size={17} />
              </Link>
              <RunStatusIcon status={r.status} size={30} />
              <div className="min-w-0 flex-1">
                <div className="flex flex-wrap items-center gap-x-2 gap-y-0.5">
                  <h1 className="text-[15px] font-semibold tracking-tight whitespace-nowrap sm:text-base">
                    {ROLE_LABEL[r.role]} run <span className="font-normal text-fg-subtle">#{r.id}</span>
                  </h1>
                  <Pill tone={RUN_STATUS_TONE[r.status]}>{r.status.replace("_", " ")}</Pill>
                  <span className="text-xs text-fg-subtle tabular-nums">{duration(r)}</span>
                </div>
                <div className="truncate text-xs text-fg-muted">
                  {r.agent} on{" "}
                  <Link to={`/p/${r.project}/issues/${r.issue}`} className="font-medium text-accent-fg hover:underline">
                    #{r.issue} {r.issue_title}
                  </Link>
                </div>
              </div>
              {r.live && (
                <Button size="sm" variant="danger" onClick={() => cancel.mutate()} disabled={cancel.isPending}>
                  <Square size={11} fill="currentColor" /> Stop
                </Button>
              )}
            </div>
            <div className="flex flex-wrap items-center gap-1.5">
              {r.pr && (
                <Link to={`/p/${r.project}/pulls/${r.pr}`} className="inline-flex items-center gap-1.5 rounded-md bg-accent-soft px-2 py-0.5 text-[11px] font-medium text-accent-fg hover:underline">
                  <GitPullRequest size={12} /> PR #{r.pr}
                </Link>
              )}
              {settings && (
                <MetaChip icon={Cpu} title="Model · effort this run used">
                  {settings}
                </MetaChip>
              )}
              <RunUsageChips totalTokens={r.total_tokens} models={r.models} costUsd={r.cost_usd} context={usage && { used: usage.used, size: usage.size }} />
              {r.container_name && (
                <MetaChip icon={Box} title="Container" className="max-sm:hidden">
                  {r.container_name}
                </MetaChip>
              )}
              {r.worktree_path && (
                <MetaChip icon={Folder} title={r.worktree_path} className="max-w-72 max-md:hidden">
                  {r.worktree_path}
                </MetaChip>
              )}
              <label className="ml-auto flex items-center gap-2 text-xs text-fg-muted">
                Follow <Switch size="sm" checked={follow} onChange={setFollow} label="Follow the transcript" />
              </label>
            </div>
            {r.error && <div className="rounded-lg bg-rose-500/8 px-2.5 py-1.5 text-xs break-words text-rose-700 dark:text-rose-300">{r.error}</div>}
          </div>
        </header>
      )}
      <div className="mx-auto w-full max-w-4xl flex-1 px-4 py-5 sm:px-6">
        <ErrorBox error={run.error} />
        <Transcript events={events} worktree={r?.worktree_path} live={!!r?.live} />
        {events.length === 0 && (
          <EmptyState icon={<Terminal size={20} />} title="No transcript yet">
            {r?.live ? "The agent is starting up…" : "This run didn't record any events."}
          </EmptyState>
        )}
        {r?.live && (
          <div className="mt-4 flex items-center gap-2 text-xs text-sky-600 dark:text-sky-400">
            <LiveDot tone="sky" /> Working…
          </div>
        )}
        <div ref={bottom} />
      </div>
      {r?.live && (
        <form
          className="sticky bottom-0 border-t border-line bg-surface/85 pb-[env(safe-area-inset-bottom)] backdrop-blur-xl"
          onSubmit={(e) => {
            e.preventDefault();
            if (msg.trim()) send.mutate();
          }}
        >
          <div className="mx-auto flex w-full max-w-4xl items-center gap-2 px-4 py-3 sm:px-6">
            <input
              className="h-11 min-w-0 flex-1 rounded-full border border-line bg-surface px-4 text-base shadow-card outline-none placeholder:text-fg-subtle focus:border-accent/60 focus:ring-3 focus:ring-accent/15 sm:text-sm"
              placeholder="Message the agent (delivered after its current turn)…"
              value={msg}
              onChange={(e) => setMsg(e.target.value)}
              aria-label="Message the agent"
            />
            <button
              className="grid h-11 w-11 shrink-0 place-items-center rounded-full bg-accent text-white shadow-card transition-transform enabled:active:scale-95 disabled:opacity-40"
              disabled={!msg.trim() || send.isPending}
              aria-label="Send"
            >
              <ArrowUp size={18} />
            </button>
          </div>
        </form>
      )}
    </div>
  );
}
