import clsx from "clsx";
import { X } from "lucide-react";
import { type ButtonHTMLAttributes, type ReactNode, useEffect, useState } from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { HOLD_LABEL, type Hold, type IssueState, STATE_LABEL } from "../api/client";

export function Button({
  variant = "default",
  size = "md",
  className,
  ...props
}: ButtonHTMLAttributes<HTMLButtonElement> & { variant?: "default" | "primary" | "danger" | "ghost" | "success"; size?: "sm" | "md" }) {
  return (
    <button
      {...props}
      className={clsx(
        "inline-flex items-center gap-1.5 rounded-md font-medium transition-colors disabled:opacity-50 disabled:cursor-not-allowed whitespace-nowrap",
        size === "sm" ? "px-2 py-1 text-xs" : "px-3 py-1.5 text-sm",
        variant === "default" &&
          "border border-zinc-300 dark:border-zinc-700 bg-white dark:bg-zinc-900 hover:bg-zinc-100 dark:hover:bg-zinc-800",
        variant === "primary" && "bg-blue-600 text-white hover:bg-blue-700",
        variant === "success" && "bg-emerald-600 text-white hover:bg-emerald-700",
        variant === "danger" && "bg-rose-600 text-white hover:bg-rose-700",
        variant === "ghost" && "hover:bg-zinc-100 dark:hover:bg-zinc-800",
        className,
      )}
    />
  );
}

const STATE_STYLE: Partial<Record<IssueState, string>> = {
  triage: "bg-violet-100 text-violet-800 dark:bg-violet-950 dark:text-violet-300",
  ready: "bg-blue-100 text-blue-800 dark:bg-blue-950 dark:text-blue-300",
  changes_requested: "bg-orange-100 text-orange-800 dark:bg-orange-950 dark:text-orange-300",
  merge_conflict: "bg-red-100 text-red-800 dark:bg-red-950 dark:text-red-300",
  ready_to_merge: "bg-emerald-100 text-emerald-800 dark:bg-emerald-950 dark:text-emerald-300",
  closed: "bg-zinc-200 text-zinc-700 dark:bg-zinc-800 dark:text-zinc-300",
  backlog: "bg-zinc-100 text-zinc-700 dark:bg-zinc-800 dark:text-zinc-300",
  in_progress: "bg-sky-100 text-sky-800 dark:bg-sky-950 dark:text-sky-300",
  in_review: "bg-indigo-100 text-indigo-800 dark:bg-indigo-950 dark:text-indigo-300",
  done: "bg-emerald-100 text-emerald-800 dark:bg-emerald-950 dark:text-emerald-300",
};

export function Pill({ className, children, title }: { className?: string; children: ReactNode; title?: string }) {
  return (
    <span title={title} className={clsx("inline-flex items-center gap-1 rounded-full px-2 py-0.5 text-[11px] font-medium leading-4 whitespace-nowrap", className)}>
      {children}
    </span>
  );
}

export function StateBadge({ state, always }: { state: IssueState; always?: boolean }) {
  // On the board, the column already says "In progress"/"In review"/"Backlog"/"Done".
  if (!always && ["backlog", "in_progress", "in_review", "done"].includes(state)) return null;
  return <Pill className={STATE_STYLE[state]}>{STATE_LABEL[state]}</Pill>;
}

const HOLD_STYLE: Record<Hold, string> = {
  needs_decision: "bg-amber-100 text-amber-900 dark:bg-amber-950 dark:text-amber-300 ring-1 ring-amber-300 dark:ring-amber-800",
  stalled: "bg-rose-100 text-rose-800 dark:bg-rose-950 dark:text-rose-300",
  paused: "bg-zinc-200 text-zinc-700 dark:bg-zinc-800 dark:text-zinc-300",
};

export function HoldBadge({ hold, reason }: { hold: Hold; reason?: string | null }) {
  const icon = hold === "needs_decision" ? "❓" : hold === "stalled" ? "⚠️" : "⏸";
  return (
    <Pill className={HOLD_STYLE[hold]} title={reason ?? undefined}>
      {icon} {HOLD_LABEL[hold]}
    </Pill>
  );
}

export function LabelChip({ name, color }: { name: string; color: string }) {
  const c = `#${color.replace("#", "")}`;
  return (
    <span
      className="inline-flex items-center rounded-full px-1.5 py-0 text-[10px] font-medium border whitespace-nowrap"
      style={{ borderColor: `${c}88`, background: `${c}22`, color: "inherit" }}
    >
      {name}
    </span>
  );
}

export function Markdown({ children, className }: { children: string; className?: string }) {
  return (
    <div className={clsx("prose-sm break-words", className)}>
      <ReactMarkdown remarkPlugins={[remarkGfm]}>{children}</ReactMarkdown>
    </div>
  );
}

export function timeAgo(iso?: string | null): string {
  if (!iso) return "";
  const s = (Date.now() - new Date(iso).getTime()) / 1000;
  if (s < -60) return `in ${fmtDur(-s)}`;
  if (s < 60) return "just now";
  return `${fmtDur(s)} ago`;
}

export function fmtDur(s: number): string {
  if (s < 3600) return `${Math.max(1, Math.round(s / 60))}m`;
  if (s < 86400) return `${Math.floor(s / 3600)}h ${Math.round((s % 3600) / 60)}m`;
  return `${Math.floor(s / 86400)}d`;
}

export function TimeAgo({ iso }: { iso?: string | null }) {
  const [, tick] = useState(0);
  useEffect(() => {
    const t = setInterval(() => tick((x) => x + 1), 30000);
    return () => clearInterval(t);
  }, []);
  if (!iso) return null;
  return (
    <time dateTime={iso} title={new Date(iso).toLocaleString()}>
      {timeAgo(iso)}
    </time>
  );
}

export function ErrorBox({ error }: { error: unknown }) {
  if (!error) return null;
  return (
    <div className="rounded-md border border-rose-300 bg-rose-50 dark:bg-rose-950/40 dark:border-rose-900 px-3 py-2 text-sm text-rose-800 dark:text-rose-300 break-words">
      {error instanceof Error ? error.message : String(error)}
    </div>
  );
}

export function Spinner({ className }: { className?: string }) {
  return <span className={clsx("inline-block h-3 w-3 animate-spin rounded-full border-2 border-current border-t-transparent", className)} />;
}

export function Modal({ open, onClose, title, children, wide }: { open: boolean; onClose: () => void; title: ReactNode; children: ReactNode; wide?: boolean }) {
  useEffect(() => {
    if (!open) return;
    const k = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("keydown", k);
    return () => window.removeEventListener("keydown", k);
  }, [open, onClose]);
  if (!open) return null;
  return (
    <div className="fixed inset-0 z-50 flex items-start justify-center bg-black/40 p-4 pt-[10vh]" onMouseDown={onClose}>
      <div
        className={clsx("w-full rounded-lg bg-white dark:bg-zinc-900 shadow-xl border border-zinc-200 dark:border-zinc-800", wide ? "max-w-3xl" : "max-w-lg")}
        onMouseDown={(e) => e.stopPropagation()}
      >
        <div className="flex items-center justify-between border-b border-zinc-200 dark:border-zinc-800 px-4 py-3">
          <h2 className="font-semibold">{title}</h2>
          <button onClick={onClose} className="text-zinc-500 hover:text-zinc-900 dark:hover:text-zinc-100">
            <X size={16} />
          </button>
        </div>
        <div className="p-4">{children}</div>
      </div>
    </div>
  );
}

export function Field({ label, children, hint }: { label: string; children: ReactNode; hint?: ReactNode }) {
  return (
    <label className="block space-y-1">
      <span className="text-xs font-medium text-zinc-600 dark:text-zinc-400">{label}</span>
      {children}
      {hint && <span className="block text-[11px] text-zinc-500">{hint}</span>}
    </label>
  );
}

export const fieldCls =
  "rounded-md border border-zinc-300 dark:border-zinc-700 bg-white dark:bg-zinc-950 px-2.5 py-1.5 text-base sm:text-sm focus:outline-none focus:ring-2 focus:ring-blue-500/40";

export const inputCls =
  "w-full rounded-md border border-zinc-300 dark:border-zinc-700 bg-white dark:bg-zinc-950 px-2.5 py-1.5 text-base sm:text-sm focus:outline-none focus:ring-2 focus:ring-blue-500/40";

export function Empty({ children }: { children: ReactNode }) {
  return <div className="text-sm text-zinc-500 italic py-6 text-center">{children}</div>;
}

export function short(sha?: string | null) {
  return sha ? sha.slice(0, 8) : "";
}
