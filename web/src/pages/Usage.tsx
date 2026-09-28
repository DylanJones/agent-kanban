import { useQuery } from "@tanstack/react-query";
import clsx from "clsx";
import { Clock } from "lucide-react";
import { useMemo, useState } from "react";
import { type S, client, unwrap } from "../api/client";
import { ErrorBox, Pill, timeAgo } from "../components/ui";

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

function Tile({ label, value, sub }: { label: string; value: string; sub?: string }) {
  return (
    <div className="rounded-lg border border-zinc-200 dark:border-zinc-800 bg-white dark:bg-zinc-900 px-4 py-3">
      <div className="text-xs text-zinc-500">{label}</div>
      <div className="text-2xl font-semibold tabular-nums">{value}</div>
      {sub && <div className="text-xs text-zinc-500">{sub}</div>}
    </div>
  );
}

function Subscriptions() {
  const subs = useQuery({ queryKey: ["usage", "subs"], queryFn: () => unwrap(client.GET("/api/usage/subscriptions")), refetchInterval: 30000 });
  if (!subs.data) return null;
  return (
    <div className="grid gap-3 md:grid-cols-3">
      {subs.data
        .filter((s) => s.windows.length > 0 || s.tokens_7d > 0 || s.paused)
        .map((s, i) => (
          <div key={s.name} className="rounded-lg border border-zinc-200 dark:border-zinc-800 bg-white dark:bg-zinc-900 p-4 space-y-2">
            <div className="flex items-center gap-2">
              <span className="h-2.5 w-2.5 rounded-sm" style={{ background: seriesColor(s.name, i) }} />
              <span className="font-medium">{s.name}</span>
              {s.plan && <Pill className="bg-zinc-100 dark:bg-zinc-800 text-zinc-600 dark:text-zinc-400">{s.plan}</Pill>}
              {s.paused && (
                <Pill className="bg-amber-100 text-amber-900 dark:bg-amber-950 dark:text-amber-300">
                  <Clock size={10} /> paused ({s.pause_kind}){s.paused_until ? ` · ${timeAgo(s.paused_until)}` : ""}
                </Pill>
              )}
            </div>
            <div className="text-xs text-zinc-500">{s.agents.join(", ")}</div>
            {s.windows.map((w) => {
              const pct = Math.max(0, Math.min(100, w.used_percent ?? 0));
              return (
                <div key={w.name} className="space-y-0.5">
                  <div className="flex justify-between text-xs">
                    <span className="text-zinc-600 dark:text-zinc-400">{w.name.replace("_", " ")} window</span>
                    <span className="tabular-nums text-zinc-700 dark:text-zinc-300">
                      {w.used_percent == null ? "—" : `${Math.round(pct)}%`}
                      {w.resets_at ? <span className="text-zinc-500"> · resets {timeAgo(w.resets_at)}</span> : null}
                    </span>
                  </div>
                  <div className="h-1.5 rounded-full bg-zinc-100 dark:bg-zinc-800 overflow-hidden" role="meter" aria-valuenow={pct} aria-valuemin={0} aria-valuemax={100}>
                    <div className={clsx("h-full rounded-full", pct >= 90 ? "bg-rose-500" : pct >= 75 ? "bg-amber-500" : "bg-zinc-500 dark:bg-zinc-400")} style={{ width: `${pct}%` }} />
                  </div>
                </div>
              );
            })}
            <div className="flex gap-4 pt-1 text-xs text-zinc-500">
              <span>
                24h <b className="text-zinc-800 dark:text-zinc-200 tabular-nums">{fmtTokens(s.tokens_24h)}</b>
              </span>
              <span>
                7d <b className="text-zinc-800 dark:text-zinc-200 tabular-nums">{fmtTokens(s.tokens_7d)}</b>
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
  if (days.length === 0) return <div className="text-sm text-zinc-500 italic py-8 text-center">No agent usage in this range yet.</div>;
  const H = 180;
  const hovered = days.find(([d]) => d === hover);
  return (
    <div className="viz-root space-y-2">
      <div className="flex items-center gap-4 text-xs text-[var(--text-secondary)]">
        <span className="font-medium text-[var(--text-primary)]">Tokens per day</span>
        {subs.map((s, i) => (
          <span key={s} className="inline-flex items-center gap-1">
            <span className="h-2.5 w-2.5 rounded-sm" style={{ background: seriesColor(s, i) }} /> {s}
          </span>
        ))}
        <span className="ml-auto tabular-nums">peak {fmtTokens(max)}</span>
      </div>
      <div className="relative">
        <div className="flex items-end gap-[2px]" style={{ height: H }} onMouseLeave={() => setHover(null)}>
          {days.map(([day, m]) => {
            const total = Object.values(m).reduce((a, b) => a + b, 0);
            return (
              <div key={day} className="group relative flex h-full min-w-0 flex-1 flex-col items-center justify-end" onMouseEnter={() => setHover(day)}>
                <div className="flex w-full max-w-10 flex-col-reverse gap-[2px] overflow-hidden rounded-t" style={{ height: `${(total / max) * 100}%` }}>
                  {subs.map((s, i) =>
                    m[s] ? <div key={s} style={{ flexGrow: m[s], flexBasis: 0, background: seriesColor(s, i), opacity: hover && hover !== day ? 0.45 : 1 }} /> : null,
                  )}
                </div>
              </div>
            );
          })}
        </div>
        <div className="mt-1 flex justify-between text-[10px] text-[var(--text-secondary)] tabular-nums">
          <span>{days[0][0]}</span>
          {days.length > 1 && <span>{days[days.length - 1][0]}</span>}
        </div>
        {hovered && Object.keys(hovered[1]).length > 0 && (
          <div className="pointer-events-none absolute right-0 top-0 rounded-md border border-zinc-200 dark:border-zinc-700 bg-white dark:bg-zinc-900 px-3 py-2 text-xs shadow-lg">
            <div className="font-medium">{hovered[0]}</div>
            {subs.map((s, i) =>
              hovered[1][s] ? (
                <div key={s} className="flex items-center gap-2 tabular-nums">
                  <span className="h-2 w-2 rounded-sm" style={{ background: seriesColor(s, i) }} />
                  <span className="text-zinc-600 dark:text-zinc-400">{s}</span>
                  <span className="ml-auto pl-4">{fmtTokens(hovered[1][s])}</span>
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
  return (
    <table className="w-full text-sm tabular-nums">
      <thead className="text-xs text-zinc-500">
        <tr className="border-b border-zinc-200 dark:border-zinc-800">
          <th className="py-2 text-left font-medium">{GROUPS.find((g) => g[0] === group)?.[1]}</th>
          <th className="py-2 text-right font-medium">Runs</th>
          <th className="py-2 text-right font-medium">Total</th>
          <th className="py-2 pl-3 text-left font-medium w-40">Share</th>
          <th className="py-2 text-right font-medium">Input</th>
          <th className="py-2 text-right font-medium">Cache read</th>
          <th className="py-2 text-right font-medium">Cache write</th>
          <th className="py-2 text-right font-medium">Output</th>
          {hasCost && <th className="py-2 text-right font-medium" title="API-equivalent cost reported by the agent">Cost*</th>}
        </tr>
      </thead>
      <tbody>
        {rows.map((r) => (
          <tr key={r.key} className="border-b border-zinc-100 dark:border-zinc-800/60">
            <td className="py-1.5 pr-3 max-w-72 truncate" title={r.key}>
              {r.key}
            </td>
            <td className="py-1.5 text-right">{r.runs}</td>
            <td className="py-1.5 text-right font-medium">{fmtTokens(r.total_tokens)}</td>
            <td className="py-1.5 pl-3">
              <div className="h-1.5 rounded-full bg-zinc-100 dark:bg-zinc-800">
                <div className="h-full rounded-full bg-[var(--series-1)]" style={{ width: `${total ? (r.total_tokens / total) * 100 : 0}%` }} />
              </div>
            </td>
            <td className="py-1.5 text-right text-zinc-600 dark:text-zinc-400">{fmtTokens(r.input_tokens)}</td>
            <td className="py-1.5 text-right text-zinc-600 dark:text-zinc-400">{fmtTokens(r.cached_input_tokens)}</td>
            <td className="py-1.5 text-right text-zinc-600 dark:text-zinc-400">{fmtTokens(r.cache_write_tokens)}</td>
            <td className="py-1.5 text-right text-zinc-600 dark:text-zinc-400">{fmtTokens(r.output_tokens)}</td>
            {hasCost && <td className="py-1.5 text-right text-zinc-600 dark:text-zinc-400">{r.cost_usd != null ? `$${r.cost_usd.toFixed(2)}` : "—"}</td>}
          </tr>
        ))}
      </tbody>
    </table>
  );
}

export default function UsagePage() {
  const [days, setDays] = useState<number | null>(30);
  const [group, setGroup] = useState("model");
  const from = days == null ? "2000-01-01" : new Date(Date.now() - days * 86400000).toISOString();
  const report = useQuery({
    queryKey: ["usage", from.slice(0, 13), group],
    queryFn: () => unwrap(client.GET("/api/usage", { params: { query: { from, group_by: group } } })),
    refetchInterval: 60000,
  });
  const r = report.data;
  const t = r?.totals;
  const cacheShare = t && t.total_tokens ? Math.round((t.cached_input_tokens / t.total_tokens) * 100) : 0;
  return (
    <div className="mx-auto max-w-6xl p-6 space-y-5">
      <style>{`
        .viz-root, .usage-page { --series-1:#2a78d6; --series-2:#eb6834; --series-3:#1baf7a; --series-4:#eda100; --series-5:#e87ba4; --series-6:#008300; --text-primary:#0b0b0b; --text-secondary:#52514e; }
        @media (prefers-color-scheme: dark) {
          .viz-root, .usage-page { --series-1:#3987e5; --series-2:#d95926; --series-3:#199e70; --series-4:#c98500; --series-5:#d55181; --series-6:#008300; --text-primary:#ffffff; --text-secondary:#c3c2b7; }
        }
      `}</style>
      <div className="usage-page space-y-5">
        <div className="flex items-center gap-3">
          <h1 className="text-xl font-semibold">Usage</h1>
          <div className="ml-auto flex rounded-md border border-zinc-300 dark:border-zinc-700 overflow-hidden text-sm">
            {RANGES.map(([label, d]) => (
              <button key={label} onClick={() => setDays(d)} className={clsx("px-3 py-1", days === d ? "bg-zinc-200 dark:bg-zinc-800 font-medium" : "hover:bg-zinc-100 dark:hover:bg-zinc-900")}>
                {label}
              </button>
            ))}
          </div>
        </div>
        <ErrorBox error={report.error} />
        {t && (
          <div className="grid grid-cols-2 gap-3 md:grid-cols-5">
            <Tile label="Total tokens" value={fmtTokens(t.total_tokens)} sub={`${t.runs} runs`} />
            <Tile label="Output tokens" value={fmtTokens(t.output_tokens)} sub={t.reasoning_tokens ? `${fmtTokens(t.reasoning_tokens)} reasoning` : undefined} />
            <Tile label="Cache reads" value={`${cacheShare}%`} sub={`${fmtTokens(t.cached_input_tokens)} of total`} />
            <Tile label="Uncached input" value={fmtTokens(t.input_tokens)} sub={`+${fmtTokens(t.cache_write_tokens)} cache writes`} />
            <Tile label="Reported cost*" value={t.cost_usd != null ? `$${t.cost_usd.toFixed(2)}` : "—"} sub="API-equivalent, where reported" />
          </div>
        )}
        <Subscriptions />
        {r && (
          <div className="rounded-lg border border-zinc-200 dark:border-zinc-800 bg-white dark:bg-zinc-900 p-4">
            <DailyChart report={r} />
          </div>
        )}
        <div className="rounded-lg border border-zinc-200 dark:border-zinc-800 bg-white dark:bg-zinc-900 p-4 space-y-3">
          <div className="flex flex-wrap gap-1">
            {GROUPS.map(([g, label]) => (
              <button key={g} onClick={() => setGroup(g)} className={clsx("rounded-md px-2.5 py-1 text-sm", group === g ? "bg-zinc-200 dark:bg-zinc-800 font-medium" : "text-zinc-600 dark:text-zinc-400 hover:bg-zinc-100 dark:hover:bg-zinc-800/60")}>
                {label}
              </button>
            ))}
          </div>
          {r && <Breakdown rows={r.rows} total={r.totals.total_tokens} group={group} />}
        </div>
        <p className="text-xs text-zinc-500">
          Totals come from each agent's session log where available (exact, per model), otherwise from what the agent reports over ACP. *Cost is
          the API-equivalent figure Claude reports during live runs; subscription usage isn't billed per token.
        </p>
      </div>
    </div>
  );
}
