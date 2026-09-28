import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import clsx from "clsx";
import { Square } from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";
import { Link, useParams } from "react-router";
import { ROLE_LABEL, type RunEvent, type RunView, api, client, unwrap } from "../api/client";
import { Transcript } from "../components/Transcript";
import { fmtTokens } from "./Usage";
import { summarizeSettings } from "../components/SessionSettings";
import { Button, Empty, ErrorBox, Pill, TimeAgo, fmtDur, fieldCls, inputCls } from "../components/ui";

export const RUN_STATUS_STYLE: Record<string, string> = {
  queued: "bg-zinc-100 text-zinc-700 dark:bg-zinc-800 dark:text-zinc-300",
  preparing: "bg-sky-100 text-sky-800 dark:bg-sky-950 dark:text-sky-300",
  running: "bg-blue-100 text-blue-800 dark:bg-blue-950 dark:text-blue-300",
  succeeded: "bg-emerald-100 text-emerald-800 dark:bg-emerald-950 dark:text-emerald-300",
  failed: "bg-rose-100 text-rose-800 dark:bg-rose-950 dark:text-rose-300",
  cancelled: "bg-zinc-200 text-zinc-700 dark:bg-zinc-800 dark:text-zinc-300",
  rate_limited: "bg-amber-100 text-amber-900 dark:bg-amber-950 dark:text-amber-300",
  interrupted: "bg-zinc-200 text-zinc-700 dark:bg-zinc-800 dark:text-zinc-300",
};

function duration(r: RunView) {
  if (!r.started_at) return "";
  const end = r.ended_at ? new Date(r.ended_at).getTime() : Date.now();
  return fmtDur((end - new Date(r.started_at).getTime()) / 1000);
}

export function RunsPage() {
  const [status, setStatus] = useState("");
  const runs = useQuery({
    queryKey: ["runs", "list", status],
    queryFn: () => unwrap(client.GET("/api/runs", { params: { query: { status: status || undefined, limit: 200 } } })),
    refetchInterval: 10000,
  });
  return (
    <div className="mx-auto max-w-6xl p-3 space-y-4 md:p-6">
      <div className="flex items-center gap-3">
        <h1 className="text-xl font-semibold">Agent runs</h1>
        <select className={clsx(fieldCls, " py-1")} value={status} onChange={(e) => setStatus(e.target.value)}>
          <option value="">All</option>
          <option value="active">Active</option>
          <option value="failed">Failed</option>
          <option value="rate_limited">Rate limited</option>
          <option value="succeeded">Succeeded</option>
        </select>
      </div>
      <ErrorBox error={runs.error} />
      <div className="rounded-md border border-zinc-200 dark:border-zinc-800 bg-white dark:bg-zinc-900 divide-y divide-zinc-100 dark:divide-zinc-800">
        {runs.data?.length === 0 && <Empty>No runs yet. Turn on the scheduler or start a run from an issue.</Empty>}
        {runs.data?.map((r) => (
          // Wide screens: one row per run. Narrow: id, status, role, agent and numbers, then the issue, then the outcome.
          <Link
            key={r.id}
            to={`/runs/${r.id}`}
            className="flex flex-wrap items-center gap-x-2 gap-y-0.5 px-3 py-2 md:gap-x-3 text-sm hover:bg-zinc-50 dark:hover:bg-zinc-800/50 md:flex-nowrap"
          >
            <span className="font-mono text-xs text-zinc-500 md:w-12">#{r.id}</span>
            <Pill className={RUN_STATUS_STYLE[r.status]}>{r.status.replace("_", " ")}</Pill>
            <span className="text-xs md:w-20">{ROLE_LABEL[r.role]}</span>
            <span className="text-xs text-zinc-500 md:w-20">{r.agent}</span>
            <span className="min-w-0 truncate max-md:order-2 max-md:basis-full md:flex-1">
              <span className="text-zinc-500">
                {r.project} #{r.issue}
              </span>{" "}
              {r.issue_title}
            </span>
            {(r.error ?? r.outcome) && (
              <span className="min-w-0 truncate text-xs text-zinc-500 max-md:order-3 max-md:basis-full md:max-w-72">{r.error ?? r.outcome}</span>
            )}
            <span className="text-right text-xs text-zinc-500 tabular-nums max-md:ml-auto md:w-14" title="tokens">
              {r.total_tokens > 0 ? fmtTokens(r.total_tokens) : ""}
            </span>
            <span className="text-right text-xs text-zinc-500 max-md:hidden md:w-14">{duration(r)}</span>
            <span className="whitespace-nowrap text-right text-xs text-zinc-500 md:w-20">
              <TimeAgo iso={r.created_at} />
            </span>
          </Link>
        ))}
      </div>
    </div>
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
    <div className="mx-auto max-w-4xl p-3 space-y-4 md:p-6">
      <ErrorBox error={run.error} />
      {r && (
        <div className="sticky top-0 z-10 -mx-3 -mt-3 border-b border-zinc-200 dark:border-zinc-800 bg-zinc-50/95 dark:bg-zinc-950/95 px-3 py-2 backdrop-blur space-y-1 md:-mx-6 md:-mt-6 md:px-6 md:py-3">
          <div className="flex flex-wrap items-center gap-x-2 gap-y-1">
            <h1 className="text-lg font-semibold whitespace-nowrap">
              {ROLE_LABEL[r.role]} run #{r.id}
            </h1>
            <Pill className={RUN_STATUS_STYLE[r.status]}>{r.status.replace("_", " ")}</Pill>
            <span className="min-w-0 text-sm text-zinc-500 max-md:order-last max-md:basis-full max-md:truncate">
              {r.agent} on{" "}
              <Link to={`/p/${r.project}/issues/${r.issue}`} className="text-blue-600">
                #{r.issue} {r.issue_title}
              </Link>
              {r.pr && (
                <>
                  {" "}
                  ·{" "}
                  <Link to={`/p/${r.project}/pulls/${r.pr}`} className="text-blue-600">
                    PR #{r.pr}
                  </Link>
                </>
              )}
            </span>
            <span className="ml-auto text-xs text-zinc-500">{duration(r)}</span>
            {r.live && (
              <Button size="sm" variant="danger" onClick={() => cancel.mutate()} disabled={cancel.isPending}>
                <Square size={11} /> Stop
              </Button>
            )}
          </div>
          <div className="flex flex-wrap gap-x-3 gap-y-0.5 text-xs text-zinc-500">
            {r.worktree_path && (
              <code title={r.worktree_path} className="min-w-0 truncate max-md:hidden">
                {r.worktree_path}
              </code>
            )}
            {r.container_name && <span>🐳 {r.container_name}</span>}
            {settings && <span title="model · effort this run used">⚙ {settings}</span>}
            {r.total_tokens > 0 && (
              <span title={r.models.join(", ")}>
                {fmtTokens(r.total_tokens)} tokens{r.cost_usd != null ? ` · $${r.cost_usd.toFixed(2)}*` : ""}
              </span>
            )}
            {usage?.used !== undefined && (
              <span>
                context {String(usage.used)}/{String(usage.size)}
              </span>
            )}
            <label className="ml-auto flex items-center gap-1">
              <input type="checkbox" checked={follow} onChange={(e) => setFollow(e.target.checked)} /> follow
            </label>
          </div>
          {r.error && <div className="text-xs text-rose-600 break-words">{r.error}</div>}
        </div>
      )}
      <div className="space-y-2">
        <Transcript events={events} worktree={r?.worktree_path} live={!!r?.live} />
        {events.length === 0 && <Empty>No transcript yet.</Empty>}
        <div ref={bottom} />
      </div>
      {r?.live && (
        <form
          className="sticky bottom-0 flex gap-2 bg-zinc-50 dark:bg-zinc-950 py-3"
          onSubmit={(e) => {
            e.preventDefault();
            if (msg.trim()) send.mutate();
          }}
        >
          <input className={inputCls} placeholder="Message the agent (delivered after its current turn)…" value={msg} onChange={(e) => setMsg(e.target.value)} />
          <Button variant="primary">Send</Button>
        </form>
      )}
    </div>
  );
}
