import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { Link, useParams } from "react-router";
import { api, client, unwrap } from "../api/client";
import { Button, Empty, ErrorBox, HoldBadge, Markdown, TimeAgo, inputCls } from "../components/ui";
import { IssueBody } from "./IssueDrawer";

function DecisionItem({ slug, n }: { slug: string; n: number }) {
  const issue = useQuery({ queryKey: ["issue", slug, n], queryFn: () => unwrap(client.GET("/api/projects/{p}/issues/{n}", { params: { path: { p: slug, n } } })) });
  const [open, setOpen] = useState(false);
  if (!issue.data) return null;
  const d = issue.data;
  return (
    <div className="rounded-lg border border-zinc-200 dark:border-zinc-800 bg-white dark:bg-zinc-900 p-4 space-y-2">
      <div className="flex items-center gap-2">
        <Link to={`/p/${slug}/issues/${n}`} className="font-medium hover:underline">
          <span className="text-zinc-500 font-mono">#{n}</span> {d.title}
        </Link>
        {d.hold && <HoldBadge hold={d.hold} reason={d.hold_reason} />}
        <span className="ml-auto text-xs text-zinc-500">
          <TimeAgo iso={d.hold_set_at} />
        </span>
        <Button size="sm" variant="ghost" onClick={() => setOpen(!open)}>
          {open ? "Hide" : "Details"}
        </Button>
      </div>
      {open ? <IssueBody issue={d} slug={slug} /> : <InlineDecision slug={slug} n={n} />}
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
      <div className="flex items-center gap-2 text-sm text-zinc-600 dark:text-zinc-400">
        <span className="flex-1">{issue.data.hold_reason}</span>
        <Button size="sm" onClick={() => clear.mutate()}>
          Clear hold & retry
        </Button>
      </div>
    );
  }
  return (
    <div className="space-y-2">
      {req && <Markdown>{req.body}</Markdown>}
      <div className="flex gap-2">
        <input className={inputCls} placeholder="Your decision…" value={answer} onChange={(e) => setAnswer(e.target.value)} />
        <Button variant="primary" disabled={!answer.trim()} onClick={() => decide.mutate()}>
          Decide
        </Button>
      </div>
      <ErrorBox error={decide.error} />
    </div>
  );
}

function Permissions() {
  const qc = useQueryClient();
  const perms = useQuery({ queryKey: ["permissions"], queryFn: () => unwrap(client.GET("/api/permission-requests", { params: { query: {} } })), refetchInterval: 10000 });
  const answer = useMutation({
    mutationFn: ({ id, option_id }: { id: number; option_id: string }) => api("POST", `/api/permission-requests/${id}`, { option_id }),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["permissions"] }),
  });
  if (!perms.data?.length) return null;
  return (
    <section className="space-y-2">
      <h2 className="font-semibold">Permission prompts</h2>
      {perms.data.map((p) => {
        const tc = p.tool_call as { title?: string; rawInput?: unknown };
        const opts = p.options as { optionId: string; name: string; kind: string }[];
        return (
          <div key={p.id} className="rounded-lg border border-amber-300 bg-amber-50 dark:bg-amber-950/30 p-3 text-sm space-y-2">
            <div>
              <Link to={`/runs/${p.run_id}`} className="text-blue-600">
                run #{p.run_id}
              </Link>{" "}
              wants to: <b>{tc.title}</b> <TimeAgo iso={p.created_at} />
            </div>
            {tc.rawInput !== undefined && <pre className="text-[11px] max-h-32 overflow-auto">{JSON.stringify(tc.rawInput, null, 2)}</pre>}
            <div className="flex gap-2">
              {opts.map((o) => (
                <Button key={o.optionId} size="sm" variant={o.kind.startsWith("allow") ? "success" : "default"} onClick={() => answer.mutate({ id: p.id, option_id: o.optionId })}>
                  {o.name}
                </Button>
              ))}
            </div>
          </div>
        );
      })}
    </section>
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
  const ready = rtm.data?.columns.flatMap((c) => c.cards) ?? [];
  const decisions = holds.data?.filter((i) => i.hold === "needs_decision") ?? [];
  const other = holds.data?.filter((i) => i.hold !== "needs_decision") ?? [];
  return (
    <div className="mx-auto max-w-4xl p-6 space-y-6">
      <Permissions />
      <section className="space-y-2">
        <h2 className="font-semibold">Needs your decision ({decisions.length})</h2>
        {decisions.length === 0 && <Empty>No open questions.</Empty>}
        {decisions.map((i) => (
          <DecisionItem key={i.number} slug={slug} n={i.number} />
        ))}
      </section>
      <section className="space-y-2">
        <h2 className="font-semibold">Ready to merge ({ready.length})</h2>
        {ready.length === 0 && <Empty>Nothing approved yet.</Empty>}
        {ready.map((c) => (
          <Link
            key={c.number}
            to={c.pr ? `/p/${slug}/pulls/${c.pr.number}` : `/p/${slug}/issues/${c.number}`}
            className="flex items-center gap-2 rounded-lg border border-emerald-200 dark:border-emerald-900 bg-white dark:bg-zinc-900 px-4 py-2 text-sm hover:border-emerald-400"
          >
            <span className="font-mono text-zinc-500">#{c.number}</span>
            <span className="flex-1">{c.title}</span>
            {c.pr && <span className="text-xs text-zinc-500">PR #{c.pr.number} →</span>}
          </Link>
        ))}
      </section>
      {other.length > 0 && (
        <section className="space-y-2">
          <h2 className="font-semibold">Stalled or paused ({other.length})</h2>
          {other.map((i) => (
            <DecisionItem key={i.number} slug={slug} n={i.number} />
          ))}
        </section>
      )}
    </div>
  );
}
