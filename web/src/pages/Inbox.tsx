import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import clsx from "clsx";
import { ChevronRight, CircleHelp, GitMerge, GitPullRequest, PartyPopper, PauseCircle, ShieldAlert } from "lucide-react";
import { type ReactNode, useState } from "react";
import { Link, useParams } from "react-router";
import { api, client, unwrap } from "../api/client";
import { Button, EmptyState, ErrorBox, HoldBadge, Markdown, Page, PageHeader, TimeAgo, inputCls } from "../components/ui";
import { IssueBody } from "./IssueDrawer";

function Group({ icon, title, count, tone, children }: { icon: ReactNode; title: string; count: number; tone: string; children: ReactNode }) {
  return (
    <section className="space-y-3">
      <h2 className="flex items-center gap-2 text-sm font-semibold">
        <span className={clsx("grid h-6 w-6 place-items-center rounded-lg", tone)}>{icon}</span>
        {title}
        <span className="rounded-full bg-surface-3 px-2 text-xs font-medium text-fg-muted tabular-nums">{count}</span>
      </h2>
      {children}
    </section>
  );
}

function DecisionItem({ slug, n }: { slug: string; n: number }) {
  const issue = useQuery({ queryKey: ["issue", slug, n], queryFn: () => unwrap(client.GET("/api/projects/{p}/issues/{n}", { params: { path: { p: slug, n } } })) });
  const [open, setOpen] = useState(false);
  if (!issue.data) return null;
  const d = issue.data;
  return (
    <div className="space-y-3 rounded-2xl border border-line bg-surface p-4 shadow-card sm:p-5">
      <div className="flex flex-wrap items-center gap-x-3 gap-y-2">
        <Link to={`/p/${slug}/issues/${n}`} className="min-w-0 flex-1 basis-60 font-medium hover:underline">
          <span className="font-mono text-sm text-fg-subtle">#{n}</span> {d.title}
        </Link>
        <div className="flex items-center gap-2">
          {d.hold && <HoldBadge hold={d.hold} reason={d.hold_reason} />}
          <TimeAgo iso={d.hold_set_at} className="text-xs text-fg-subtle" />
          <Button size="sm" variant="ghost" onClick={() => setOpen(!open)}>
            {open ? "Hide details" : "Details"}
          </Button>
        </div>
      </div>
      {open ? (
        <div className="border-t border-line pt-4">
          <IssueBody issue={d} slug={slug} />
        </div>
      ) : (
        <InlineDecision slug={slug} n={n} />
      )}
    </div>
  );
}

function InlineDecision({ slug, n }: { slug: string; n: number }) {
  const qc = useQueryClient();
  const issue = useQuery({ queryKey: ["issue", slug, n], queryFn: () => unwrap(client.GET("/api/projects/{p}/issues/{n}", { params: { path: { p: slug, n } } })) });
  const [answer, setAnswer] = useState("");
  const decide = useMutation({
    mutationFn: () => api("POST", `/api/projects/${slug}/issues/${n}/decision`, { answer }),
    onSuccess: () => qc.invalidateQueries(),
  });
  const clear = useMutation({ mutationFn: () => api("DELETE", `/api/projects/${slug}/issues/${n}/hold`), onSuccess: () => qc.invalidateQueries() });
  if (!issue.data) return null;
  const req = [...issue.data.comments].reverse().find((c) => c.kind === "decision_request");
  if (issue.data.hold !== "needs_decision") {
    return (
      <div className="flex flex-col gap-3 text-sm text-fg-muted sm:flex-row sm:items-center">
        <span className="flex-1">{issue.data.hold_reason}</span>
        <Button size="sm" onClick={() => clear.mutate()} className="justify-center">
          Clear hold & retry
        </Button>
      </div>
    );
  }
  return (
    <div className="space-y-3">
      {req && (
        <div className="rounded-xl border border-amber-500/30 bg-amber-500/6 px-3.5 py-3">
          <Markdown>{req.body}</Markdown>
        </div>
      )}
      <form
        className="flex flex-col gap-2 sm:flex-row"
        onSubmit={(e) => {
          e.preventDefault();
          if (answer.trim()) decide.mutate();
        }}
      >
        <input className={inputCls} placeholder="Your decision…" value={answer} onChange={(e) => setAnswer(e.target.value)} aria-label="Your decision" />
        <Button variant="primary" className="justify-center" disabled={!answer.trim() || decide.isPending}>
          Decide
        </Button>
      </form>
      <ErrorBox error={decide.error} />
    </div>
  );
}

function usePermissions() {
  return useQuery({ queryKey: ["permissions"], queryFn: () => unwrap(client.GET("/api/permission-requests", { params: { query: {} } })), refetchInterval: 10000 });
}

function Permissions() {
  const qc = useQueryClient();
  const perms = usePermissions();
  const answer = useMutation({
    mutationFn: ({ id, option_id }: { id: number; option_id: string }) => api("POST", `/api/permission-requests/${id}`, { option_id }),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["permissions"] }),
  });
  if (!perms.data?.length) return null;
  return (
    <Group icon={<ShieldAlert size={14} />} title="Permission prompts" count={perms.data.length} tone="bg-amber-500/15 text-amber-600 dark:text-amber-400">
      {perms.data.map((p) => {
        const tc = p.tool_call as { title?: string; rawInput?: unknown };
        const opts = p.options as { optionId: string; name: string; kind: string }[];
        return (
          <div key={p.id} className="space-y-3 rounded-2xl border border-amber-500/40 bg-amber-500/6 p-4 text-sm">
            <div className="flex flex-wrap items-baseline gap-x-1.5">
              <Link to={`/runs/${p.run_id}`} className="font-medium text-accent-fg hover:underline">
                Run #{p.run_id}
              </Link>
              wants to <b className="font-semibold">{tc.title}</b>
              <TimeAgo iso={p.created_at} className="text-xs text-fg-subtle" />
            </div>
            {tc.rawInput !== undefined && (
              <pre className="max-h-40 overflow-auto rounded-lg border border-line bg-surface p-2.5 font-mono text-[11px]">{JSON.stringify(tc.rawInput, null, 2)}</pre>
            )}
            <div className="flex flex-wrap gap-2">
              {opts.map((o) => (
                <Button key={o.optionId} size="sm" variant={o.kind.startsWith("allow") ? "success" : "default"} onClick={() => answer.mutate({ id: p.id, option_id: o.optionId })}>
                  {o.name}
                </Button>
              ))}
            </div>
          </div>
        );
      })}
    </Group>
  );
}

export default function InboxPage() {
  const { slug = "" } = useParams();
  const holds = useQuery({
    queryKey: ["inbox", "holds", slug],
    queryFn: () => unwrap(client.GET("/api/projects/{p}/issues", { params: { path: { p: slug }, query: { hold: "any" } } })),
  });
  const rtm = useQuery({
    queryKey: ["inbox", "rtm", slug],
    queryFn: () => unwrap(client.GET("/api/projects/{p}/board", { params: { path: { p: slug }, query: { badge: "ready_to_merge" } } })),
  });
  const perms = usePermissions();
  const ready = rtm.data?.columns.flatMap((c) => c.cards) ?? [];
  const decisions = holds.data?.filter((i) => i.hold === "needs_decision") ?? [];
  const other = holds.data?.filter((i) => i.hold !== "needs_decision") ?? [];
  const loaded = holds.data && rtm.data;
  const allClear = loaded && ready.length === 0 && decisions.length === 0 && other.length === 0 && !perms.data?.length;
  return (
    <Page width="md">
      <PageHeader title="Inbox" subtitle="Decisions, approvals and merges waiting on you." />
      <ErrorBox error={holds.error ?? rtm.error} />
      {allClear ? (
        <div className="rounded-2xl border border-line bg-surface shadow-card">
          <EmptyState icon={<PartyPopper size={20} />} title="You're all caught up">
            Nothing needs a decision or a merge right now. Agents will drop things here when they need you.
          </EmptyState>
        </div>
      ) : (
        <div className="space-y-8">
          <Permissions />
          {decisions.length > 0 && (
            <Group icon={<CircleHelp size={14} />} title="Needs your decision" count={decisions.length} tone="bg-amber-500/15 text-amber-600 dark:text-amber-400">
              {decisions.map((i) => (
                <DecisionItem key={i.number} slug={slug} n={i.number} />
              ))}
            </Group>
          )}
          {ready.length > 0 && (
            <Group icon={<GitMerge size={14} />} title="Ready to merge" count={ready.length} tone="bg-emerald-500/15 text-emerald-600 dark:text-emerald-400">
              <div className="divide-y divide-line overflow-hidden rounded-2xl border border-line bg-surface shadow-card">
                {ready.map((c) => (
                  <Link
                    key={c.number}
                    to={c.pr ? `/p/${slug}/pulls/${c.pr.number}` : `/p/${slug}/issues/${c.number}`}
                    className="group flex items-center gap-3 px-4 py-3 text-sm transition-colors hover:bg-surface-2/60"
                  >
                    <GitPullRequest size={16} className="shrink-0 text-emerald-500" />
                    <div className="min-w-0 flex-1">
                      <div className="truncate font-medium">{c.title}</div>
                      <div className="text-xs text-fg-subtle">
                        <span className="font-mono">#{c.number}</span>
                        {c.pr && <> · PR #{c.pr.number}</>}
                        {c.pr?.approved && <span className="text-emerald-600 dark:text-emerald-400"> · approved</span>}
                      </div>
                    </div>
                    <span className="hidden text-xs font-medium text-accent-fg sm:inline">Review & merge</span>
                    <ChevronRight size={16} className="shrink-0 text-fg-subtle" />
                  </Link>
                ))}
              </div>
            </Group>
          )}
          {other.length > 0 && (
            <Group icon={<PauseCircle size={14} />} title="Stalled or paused" count={other.length} tone="bg-surface-3 text-fg-muted">
              {other.map((i) => (
                <DecisionItem key={i.number} slug={slug} n={i.number} />
              ))}
            </Group>
          )}
        </div>
      )}
    </Page>
  );
}
