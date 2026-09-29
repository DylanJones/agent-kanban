import { keepPreviousData, useQuery } from "@tanstack/react-query";
import clsx from "clsx";
import { Clock } from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";
import { type S, client, unwrap } from "../api/client";
import { ErrorBox, Page, PageHeader, Pill, Segmented, timeAgo } from "../components/ui";

type Report = S["UsageReport"];
type Row = S["UsageRow"];

export function fmtTokens(n: number): string {
  if (n >= 1e9) return `${(n / 1e9).toFixed(n >= 1e10 ? 0 : 1)}B`;
  if (n >= 1e6) return `${(n / 1e6).toFixed(n >= 1e7 ? 0 : 1)}M`;
  if (n >= 1e3) return `${(n / 1e3).toFixed(n >= 1e4 ? 0 : 1)}k`;
  return String(n);
}

const RANGES: [string, number | null][] = [
  ["24h", 1],
  ["7d", 7],
  ["30d", 30],
  ["90d", 90],
  ["All", null],
];

const GROUPS: [string, string][] = [
  ["model", "Model"],
  ["subscription", "Subscription"],
  ["role", "Pipeline phase"],
  ["agent", "Agent"],
  ["project", "Project"],
  ["issue", "Issue"],
  ["day", "Day"],
];

/** Categorical slots follow the subscription, never its rank (reference palette slots 1–3, then fallbacks). */
const SERIES_VAR: Record<string, string> = { claude: "var(--series-1)", codex: "var(--series-2)", opencode: "var(--series-3)" };
const seriesColor = (name: string, i: number) => SERIES_VAR[name] ?? ["var(--series-4)", "var(--series-5)", "var(--series-6)"][i % 3];

const card = "rounded-xl border border-line bg-surface shadow-card";

function Tile({ label, value, sub }: { label: string; value: string; sub?: string }) {
  return (
    <div className={clsx(card, "px-4 py-3.5")}>
      <div className="text-xs font-medium text-fg-muted">{label}</div>
      <div className="mt-1 text-2xl font-semibold tracking-tight tabular-nums">{value}</div>
      {sub && <div className="mt-0.5 truncate text-xs text-fg-subtle">{sub}</div>}
    </div>
  );
}

function Subscriptions() {
  const subs = useQuery({ queryKey: ["usage", "subs"], queryFn: () => unwrap(client.GET("/api/usage/subscriptions")), refetchInterval: 30000 });
  if (!subs.data) return null;
  const shown = subs.data.filter((s) => s.windows.length > 0 || s.tokens_7d > 0 || s.paused);
  if (shown.length === 0) return null;
  return (
    <div className="grid gap-3 md:grid-cols-3">
      {shown.map((s, i) => (
        <div key={s.name} className={clsx(card, "space-y-3 p-4")}>
          <div className="flex flex-wrap items-center gap-2">
            <span className="h-2.5 w-2.5 rounded-full" style={{ background: seriesColor(s.name, i) }} />
            <span className="font-semibold">{s.name}</span>
            {s.plan && <Pill>{s.plan}</Pill>}
            {s.paused && (
              <Pill tone="amber">
                <Clock size={10} /> paused ({s.pause_kind}){s.paused_until ? ` · ${timeAgo(s.paused_until)}` : ""}
              </Pill>
            )}
          </div>
          <div className="text-xs text-fg-subtle">{s.agents.join(", ")}</div>
          {s.windows.map((w) => {
            const pct = Math.max(0, Math.min(100, w.used_percent ?? 0));
            return (
              <div key={w.name} className="space-y-1">
                <div className="flex justify-between gap-2 text-xs">
                  <span className="text-fg-muted">{w.name.replace("_", " ")} window</span>
                  <span className="text-fg tabular-nums">
                    {w.used_percent == null ? "—" : `${Math.round(pct)}%`}
                    {w.resets_at ? <span className="text-fg-subtle"> · resets {timeAgo(w.resets_at)}</span> : null}
                  </span>
                </div>
                <div className="h-1.5 overflow-hidden rounded-full bg-surface-3" role="meter" aria-valuenow={pct} aria-valuemin={0} aria-valuemax={100}>
                  <div className={clsx("h-full rounded-full transition-[width]", pct >= 90 ? "bg-rose-500" : pct >= 75 ? "bg-amber-500" : "bg-accent")} style={{ width: `${pct}%` }} />
                </div>
              </div>
            );
          })}
          <div className="flex gap-4 border-t border-line pt-3 text-xs text-fg-subtle">
            <span>
              24h <b className="font-semibold text-fg tabular-nums">{fmtTokens(s.tokens_24h)}</b>
            </span>
            <span>
              7d <b className="font-semibold text-fg tabular-nums">{fmtTokens(s.tokens_7d)}</b>
            </span>
            <span className="ml-auto">updated {timeAgo(s.updated_at)}</span>
          </div>
        </div>
      ))}
    </div>
  );
}

function DailyChart({ report }: { report: Report }) {
  const [hover, setHover] = useState<string | null>(null);
  const { days, subs, max } = useMemo(() => {
    const subs = [...new Set(report.daily.map((d) => d.subscription))].sort((a, b) => (SERIES_VAR[a] ? 0 : 1) - (SERIES_VAR[b] ? 0 : 1) || a.localeCompare(b));
    const byDay = new Map<string, Record<string, number>>();
    for (const d of report.daily) {
      const m = byDay.get(d.day) ?? {};
      m[d.subscription] = (m[d.subscription] ?? 0) + d.total_tokens;
      byDay.set(d.day, m);
    }
    // One slot per calendar day in the range (empty days included), so bars keep their scale.
    const localDay = (d: Date) => `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
    const first = [...byDay.keys()].sort()[0];
    const start = new Date(Math.max(new Date(report.from).getTime(), first ? new Date(`${first}T00:00:00`).getTime() - 6 * 86400000 : 0));
    const end = new Date(Math.min(new Date(report.to).getTime(), Date.now()));
    const slots: string[] = [];
    for (let d = new Date(start.getFullYear(), start.getMonth(), start.getDate()); d <= end && slots.length < 120; d.setDate(d.getDate() + 1)) {
      slots.push(localDay(d));
    }
    for (const k of byDay.keys()) if (!slots.includes(k)) slots.push(k);
    slots.sort();
    const days = slots.map((d) => [d, byDay.get(d) ?? {}] as [string, Record<string, number>]);
    const max = Math.max(1, ...days.map(([, m]) => Object.values(m).reduce((a, b) => a + b, 0)));
    return { days, subs, max };
  }, [report.daily, report.from, report.to]);
  if (days.length === 0) return <div className="py-10 text-center text-sm text-fg-subtle">No agent usage in this range yet.</div>;
  const H = 180;
  const hovered = days.find(([d]) => d === hover);
  return (
    <div className="viz-root space-y-3">
      <div className="flex flex-wrap items-center gap-x-4 gap-y-1 text-xs text-fg-muted">
        <span className="text-sm font-semibold text-fg">Tokens per day</span>
        {subs.map((s, i) => (
          <span key={s} className="inline-flex items-center gap-1.5">
            <span className="h-2.5 w-2.5 rounded-full" style={{ background: seriesColor(s, i) }} /> {s}
          </span>
        ))}
        <span className="ml-auto tabular-nums">peak {fmtTokens(max)}</span>
      </div>
      <div className="relative">
        <div className="flex items-end gap-[2px] border-b border-line" style={{ height: H }} onMouseLeave={() => setHover(null)}>
          {days.map(([day, m]) => {
            const total = Object.values(m).reduce((a, b) => a + b, 0);
            return (
              <div key={day} className="group relative flex h-full min-w-0 flex-1 flex-col items-center justify-end" onMouseEnter={() => setHover(day)} onTouchStart={() => setHover(day)}>
                <div className="flex w-full max-w-10 flex-col-reverse gap-[2px] overflow-hidden rounded-t-md" style={{ height: `${(total / max) * 100}%` }}>
                  {subs.map((s, i) =>
                    m[s] ? <div key={s} style={{ flexGrow: m[s], flexBasis: 0, background: seriesColor(s, i), opacity: hover && hover !== day ? 0.4 : 1 }} className="transition-opacity" /> : null,
                  )}
                </div>
              </div>
            );
          })}
        </div>
        <div className="mt-1.5 flex justify-between text-[10px] text-fg-subtle tabular-nums">
          <span>{days[0][0]}</span>
          {days.length > 1 && <span>{days[days.length - 1][0]}</span>}
        </div>
        {hovered && Object.keys(hovered[1]).length > 0 && (
          <div className="pointer-events-none absolute top-0 right-0 animate-fade-in rounded-xl border border-line bg-surface/95 px-3 py-2 text-xs shadow-raised backdrop-blur">
            <div className="mb-1 font-semibold">{hovered[0]}</div>
            {subs.map((s, i) =>
              hovered[1][s] ? (
                <div key={s} className="flex items-center gap-2 tabular-nums">
                  <span className="h-2 w-2 rounded-full" style={{ background: seriesColor(s, i) }} />
                  <span className="text-fg-muted">{s}</span>
                  <span className="ml-auto pl-4 font-medium">{fmtTokens(hovered[1][s])}</span>
                </div>
              ) : null,
            )}
          </div>
        )}
      </div>
    </div>
  );
}

function Breakdown({ rows, total, group }: { rows: Row[]; total: number; group: string }) {
  const hasCost = rows.some((r) => r.cost_usd != null);
  const th = "py-2.5 px-3 text-right font-medium whitespace-nowrap";
  const td = "py-2 px-3 text-right text-fg-muted";
  return (
    <div className="-mx-4 overflow-x-auto sm:-mx-5">
      <table className="w-full min-w-[640px] text-sm tabular-nums">
        <thead className="text-xs text-fg-subtle">
          <tr className="border-b border-line">
            <th className="py-2.5 pr-3 pl-4 text-left font-medium sm:pl-5">{GROUPS.find((g) => g[0] === group)?.[1]}</th>
            <th className={th}>Runs</th>
            <th className={th}>Total</th>
            <th className="w-36 px-3 py-2.5 text-left font-medium">Share</th>
            <th className={th}>Input</th>
            <th className={th}>Cache read</th>
            <th className={th}>Cache write</th>
            <th className={clsx(th, !hasCost && "pr-4 sm:pr-5")}>Output</th>
            {hasCost && (
              <th className={clsx(th, "pr-4 sm:pr-5")} title="API-equivalent cost reported by the agent">
                Cost*
              </th>
            )}
          </tr>
        </thead>
        <tbody className="divide-y divide-line/70">
          {rows.map((r) => (
            <tr key={r.key} className="transition-colors hover:bg-surface-2/50">
              <td className="max-w-72 truncate py-2 pr-3 pl-4 font-medium sm:pl-5" title={r.key}>
                {r.key}
              </td>
              <td className={td}>{r.runs}</td>
              <td className="px-3 py-2 text-right font-semibold">{fmtTokens(r.total_tokens)}</td>
              <td className="px-3 py-2">
                <div className="h-1.5 rounded-full bg-surface-3">
                  <div className="h-full rounded-full bg-[var(--series-1)]" style={{ width: `${total ? (r.total_tokens / total) * 100 : 0}%` }} />
                </div>
              </td>
              <td className={td}>{fmtTokens(r.input_tokens)}</td>
              <td className={td}>{fmtTokens(r.cached_input_tokens)}</td>
              <td className={td}>{fmtTokens(r.cache_write_tokens)}</td>
              <td className={clsx(td, !hasCost && "pr-4 sm:pr-5")}>{fmtTokens(r.output_tokens)}</td>
              {hasCost && <td className={clsx(td, "pr-4 sm:pr-5")}>{r.cost_usd != null ? `$${r.cost_usd.toFixed(2)}` : "—"}</td>}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

export default function UsagePage() {
  const [days, setDays] = useState<number | null>(30);
  const [group, setGroup] = useState("model");
  const from = days == null ? "2000-01-01" : new Date(Date.now() - days * 86400000).toISOString();
  const report = useQuery({
    queryKey: ["usage", from.slice(0, 13), group],
    queryFn: () => unwrap(client.GET("/api/usage", { params: { query: { from, group_by: group } } })),
    placeholderData: keepPreviousData,
    refetchInterval: 60000,
  });
  // React Query clears `data` on a failed fetch even with keepPreviousData, so a rejected
  // uncached tab would otherwise unmount the tiles/chart/table and collapse the page just like
  // the unpatched loading state did. Retain the last successful report ourselves and fall back
  // to it while the current query is in error, alongside the ErrorBox reporting the failure.
  const previousReportRef = useRef<Report | undefined>(undefined);
  useEffect(() => {
    if (report.data) previousReportRef.current = report.data;
  }, [report.data]);
  const r = report.data ?? (report.isError ? previousReportRef.current : undefined);
  // While a new group/range fetches (or is paused offline) or the latest fetch failed, keep
  // rendering the previous report dimmed instead of unmounting it, so the page doesn't shrink
  // and jump the scroll position back to the top.
  const stale = report.isPlaceholderData || report.isError;
  const t = r?.totals;
  const cacheShare = t && t.total_tokens ? Math.round((t.cached_input_tokens / t.total_tokens) * 100) : 0;
  return (
    <Page width="xl">
      <style>{`
        .viz-root, .usage-page { --series-1:#2a78d6; --series-2:#eb6834; --series-3:#1baf7a; --series-4:#eda100; --series-5:#e87ba4; --series-6:#008300; }
        .dark .viz-root, .dark .usage-page { --series-1:#3987e5; --series-2:#d95926; --series-3:#199e70; --series-4:#c98500; --series-5:#d55181; --series-6:#008300; }
      `}</style>
      <div className="usage-page space-y-5">
        <PageHeader
          className="mb-1"
          title="Usage"
          subtitle="Tokens your agents spent, by subscription, model and phase."
          actions={<Segmented label="Range" value={days} onChange={setDays} options={RANGES.map(([label, d]) => ({ value: d, label }))} />}
        />
        <ErrorBox error={report.error} />
        {t && (
          <div className={clsx("grid grid-cols-2 gap-3 transition-opacity lg:grid-cols-5", stale && "opacity-60")}>
            <Tile label="Total tokens" value={fmtTokens(t.total_tokens)} sub={`${t.runs} runs`} />
            <Tile label="Output tokens" value={fmtTokens(t.output_tokens)} sub={t.reasoning_tokens ? `${fmtTokens(t.reasoning_tokens)} reasoning` : undefined} />
            <Tile label="Cache reads" value={`${cacheShare}%`} sub={`${fmtTokens(t.cached_input_tokens)} of total`} />
            <Tile label="Uncached input" value={fmtTokens(t.input_tokens)} sub={`+${fmtTokens(t.cache_write_tokens)} cache writes`} />
            <Tile label="Reported cost*" value={t.cost_usd != null ? `$${t.cost_usd.toFixed(2)}` : "—"} sub="API-equivalent, where reported" />
          </div>
        )}
        <Subscriptions />
        {r && (
          <div className={clsx(card, "p-4 transition-opacity sm:p-5", stale && "opacity-60")}>
            <DailyChart report={r} />
          </div>
        )}
        <div className={clsx(card, "space-y-3 overflow-hidden p-4 transition-opacity sm:p-5", stale && "opacity-60")}>
          <div className="flex flex-wrap items-center gap-3">
            <span className="text-sm font-semibold">Breakdown</span>
            <Segmented size="sm" label="Group by" value={group} onChange={setGroup} options={GROUPS.map(([g, label]) => ({ value: g, label }))} />
          </div>
          {r && <Breakdown rows={r.rows} total={r.totals.total_tokens} group={r.group_by} />}
        </div>
        <p className="text-xs leading-relaxed text-fg-subtle">
          Token totals add up every request an agent made: each request's input (including cache reads) plus its output. Agents resend the conversation on every turn, so a
          run's total is usually many times its context window — it measures work done, not what was loaded at once. Totals come from each agent's session log where available (exact, per model), otherwise from what the agent reports over ACP. *Cost is the API-equivalent figure Claude reports
          during live runs; subscription usage isn't billed per token.
        </p>
      </div>
    </Page>
  );
}
