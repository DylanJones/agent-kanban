import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import clsx from "clsx";
import { AlertTriangle, RefreshCw } from "lucide-react";
import { useEffect, useState } from "react";
import { type S, client, unwrap } from "../api/client";
import { Button, ErrorBox, Pill, TimeAgo, short } from "../components/ui";

type BuildStatus = S["BuildStatus"];

function useBuildStatus(fast: boolean) {
  return useQuery({
    queryKey: ["build-status"],
    queryFn: () => unwrap(client.GET("/api/build-status")),
    refetchInterval: fast ? 1000 : 15000,
  });
}

function useLatestBuildJob() {
  return useQuery({
    queryKey: ["jobs", "server.build"],
    queryFn: () => unwrap(client.GET("/api/jobs", { params: { query: { kind: "server.build", limit: 1 } } })),
    refetchInterval: (q) => (q.state.data?.[0]?.status === "running" || q.state.data?.[0]?.status === "queued" ? 1000 : false),
  });
}

function JobLog() {
  const jobs = useLatestBuildJob();
  const j = jobs.data?.[0];
  if (!j) return null;
  return (
    <div className="space-y-1">
      <div className="text-xs text-zinc-500">
        Last build:{" "}
        <Pill className={j.status === "succeeded" ? "bg-emerald-100 text-emerald-800" : j.status === "failed" ? "bg-rose-100 text-rose-800" : "bg-sky-100 text-sky-800"}>
          {j.status}
        </Pill>{" "}
        <TimeAgo iso={j.created_at} />
      </div>
      {j.log && <pre className="max-h-72 overflow-auto rounded bg-zinc-900 text-zinc-300 p-2 text-[11px]">{j.log.split("\n").slice(-200).join("\n")}</pre>}
      {j.error && <div className="text-xs text-rose-600">{j.error}</div>}
    </div>
  );
}

function CommitsBehind({ status }: { status: BuildStatus }) {
  if (status.commits_behind == null) {
    if (status.head_sha && status.head_sha !== status.build_sha) {
      return <span className="text-zinc-500">Running {short(status.build_sha)}; can't tell how far behind {short(status.head_sha)} it is.</span>;
    }
    return <span className="text-emerald-600 dark:text-emerald-400">Up to date ({short(status.build_sha)}).</span>;
  }
  if (status.commits_behind === 0) {
    return <span className="text-emerald-600 dark:text-emerald-400">Up to date ({short(status.build_sha)}).</span>;
  }
  return (
    <span className="text-amber-700 dark:text-amber-400">
      {status.commits_behind} new commit{status.commits_behind === 1 ? "" : "s"} since this build ({short(status.build_sha)} → {short(status.head_sha)}).
    </span>
  );
}

export default function ServerPage() {
  const qc = useQueryClient();
  // The job this tab triggered (if any), and the server instance running when it was triggered:
  // tracked separately from `restart_pending` so a build failure can be distinguished from an
  // in-progress or completed restart, and so a restart is detected even when it produces the same
  // `build_sha` (e.g. rebuilding with no new commits still yields a new process/instance_id).
  const [awaitingJobId, setAwaitingJobId] = useState<number | null>(null);
  const [awaitingInstanceId, setAwaitingInstanceId] = useState<string | null>(null);
  // The moment (Date.now()) the current attempt started being awaited: a `build-status` response
  // fetched before this is a leftover from the *previous* attempt (the server clears its
  // `deploy_error` synchronously as part of admitting a new rebuild, before this component ever
  // sets this), and must not be attributed to the one being awaited now (thread #126 on issue
  // #12: otherwise a stale cached error instantly re-fails a freshly started retry).
  const [awaitingSince, setAwaitingSince] = useState<number | null>(null);
  // Latched once, rather than read live from `status.data.deploy_error`: that field clears as
  // soon as the *next* rebuild begins, but the message needs to stick around on screen until then.
  const [deployError, setDeployError] = useState<string | null>(null);
  const busyPolling = awaitingJobId != null || awaitingInstanceId != null;
  const status = useBuildStatus(busyPolling);
  const job = useLatestBuildJob();
  const trackedJob = awaitingJobId != null && job.data?.[0]?.id === awaitingJobId ? job.data[0] : undefined;
  const buildFailed = trackedJob?.status === "failed" || deployError != null;

  // The job row's own `status`/`error` can't be relied on alone: a completion write that itself
  // fails to persist (e.g. a full disk) can leave the row `running` forever with no error, which
  // would otherwise strand this page on "Restarting…" with no way to retry (thread #122 on issue
  // #12). `deploy_error` is in-memory on the server and can't fail to persist the same way.
  //
  // Strictly *after* `awaitingSince`, not `>=`: a response that finished in the very same
  // `Date.now()` tick as the attempt started being awaited (e.g. a poll already in flight when the
  // retry was admitted) is exactly as likely to be the stale, pre-admission value as a fresh one,
  // and must not be trusted (thread #126 on issue #12 — a tied timestamp let a stale cached error
  // instantly re-fail a freshly started retry). A same-tick fresh error just shows up on the next
  // poll instead.
  useEffect(() => {
    if (busyPolling && status.data?.deploy_error && (awaitingSince == null || status.dataUpdatedAt > awaitingSince)) {
      setDeployError(status.data.deploy_error);
    }
  }, [busyPolling, status.data?.deploy_error, status.dataUpdatedAt, awaitingSince]);

  // Notice a restart even if this tab didn't trigger it (another tab did, or the page was
  // reloaded mid-build), so it isn't stuck never checking for the server coming back.
  useEffect(() => {
    if (status.data?.restart_pending && awaitingInstanceId == null) {
      setAwaitingInstanceId(status.data.instance_id);
    }
  }, [status.data?.restart_pending, status.data?.instance_id, awaitingInstanceId]);

  useEffect(() => {
    if (awaitingInstanceId != null && status.data && status.data.instance_id !== awaitingInstanceId) {
      // The server came back up as a new process: reload for fresh assets and data.
      window.location.reload();
    }
  }, [awaitingInstanceId, status.data]);

  useEffect(() => {
    if (buildFailed) {
      // The build we were waiting on failed (or its outcome couldn't even be recorded): nothing
      // to restart into, so stop waiting and let the buttons come back. A job-row error is shown
      // via JobLog, which reads the same job; `deployError` (below) covers the case where the job
      // row itself never got a terminal status to show one.
      setAwaitingJobId(null);
      setAwaitingInstanceId(null);
    }
  }, [buildFailed]);

  const rebuild = useMutation({
    mutationFn: (mode: "now" | "drain") => unwrap(client.POST("/api/build-status/rebuild", { body: { mode } })),
    onSuccess: (startedJob) => {
      setAwaitingJobId(startedJob.id);
      if (status.data) setAwaitingInstanceId(status.data.instance_id);
      setDeployError(null);
      setAwaitingSince(Date.now());
      qc.invalidateQueries({ queryKey: ["jobs", "server.build"] });
      qc.invalidateQueries({ queryKey: ["build-status"] });
    },
  });

  if (status.isLoading) return null;
  if (!status.data) return <ErrorBox error={status.error} />;
  const s = status.data;
  const restarting = (awaitingJobId != null && !buildFailed) || s.restart_pending;
  const busy = restarting || rebuild.isPending;

  return (
    <div className="mx-auto max-w-2xl space-y-4 p-4">
      <h1 className="text-lg font-semibold">Server</h1>
      <section className="space-y-3 rounded-lg border border-zinc-200 dark:border-zinc-800 bg-white dark:bg-zinc-900 p-4">
        <div className="text-sm">
          <CommitsBehind status={s} />
        </div>
        <div className="text-xs text-zinc-500">
          Built <TimeAgo iso={s.build_time} /> from {short(s.build_sha)}.
        </div>
        {s.dirty && (
          <div className="flex items-center gap-1.5 text-xs text-amber-700 dark:text-amber-400">
            <AlertTriangle size={13} /> The checkout has uncommitted changes; a rebuild would include them.
          </div>
        )}
        {restarting ? (
          <div className="flex items-center gap-2 text-sm text-sky-700 dark:text-sky-400">
            <RefreshCw size={14} className="animate-spin" />
            {s.dispatch_paused && s.active_runs > 0
              ? `Waiting for ${s.active_runs} active run${s.active_runs === 1 ? "" : "s"} to finish, then restarting…`
              : "Restarting… this page will reload automatically."}
          </div>
        ) : (
          <div className="flex flex-wrap gap-2">
            <Button variant="primary" disabled={busy} onClick={() => rebuild.mutate("now")} title="Rebuild, then restart immediately. Active agent runs are interrupted and resume after the restart.">
              <RefreshCw size={13} className={clsx(rebuild.isPending && "animate-spin")} /> Rebuild &amp; restart now
            </Button>
            <Button disabled={busy} onClick={() => rebuild.mutate("drain")} title="Rebuild, stop dispatching new runs, and restart once the active ones finish.">
              Rebuild &amp; restart when idle
            </Button>
          </div>
        )}
        {deployError && <div className="text-xs text-rose-600">{deployError}</div>}
        {rebuild.isError && <ErrorBox error={rebuild.error} />}
      </section>
      <section className="rounded-lg border border-zinc-200 dark:border-zinc-800 bg-white dark:bg-zinc-900 p-4">
        <JobLog />
      </section>
    </div>
  );
}
