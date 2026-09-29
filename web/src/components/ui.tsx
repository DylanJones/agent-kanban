import clsx from "clsx";
import { AlertCircle, AlertTriangle, CircleHelp, Pause, X } from "lucide-react";
import {
  type ButtonHTMLAttributes,
  type ComponentProps,
  createContext,
  type HTMLAttributes,
  type ReactNode,
  useCallback,
  useContext,
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
} from "react";
import ReactMarkdown, { type Components, type ExtraProps } from "react-markdown";
import remarkGfm from "remark-gfm";
import { HOLD_LABEL, type Hold, type IssueState, STATE_LABEL } from "../api/client";

// ---------- tones ----------

export type Tone = "neutral" | "accent" | "sky" | "blue" | "indigo" | "violet" | "green" | "amber" | "orange" | "red";

/** Soft background + readable text for each tone, tuned for both themes. */
export const TONE: Record<Tone, string> = {
  neutral: "bg-surface-3 text-fg-muted",
  accent: "bg-accent-soft text-accent-fg",
  sky: "bg-sky-500/12 text-sky-700 dark:text-sky-300",
  blue: "bg-blue-500/12 text-blue-700 dark:text-blue-300",
  indigo: "bg-indigo-500/14 text-indigo-700 dark:text-indigo-300",
  violet: "bg-violet-500/12 text-violet-700 dark:text-violet-300",
  green: "bg-emerald-500/12 text-emerald-700 dark:text-emerald-300",
  amber: "bg-amber-500/15 text-amber-800 dark:text-amber-300",
  orange: "bg-orange-500/12 text-orange-700 dark:text-orange-300",
  red: "bg-rose-500/12 text-rose-700 dark:text-rose-300",
};

/** A solid dot colour per tone (status dots, legend swatches). */
export const TONE_DOT: Record<Tone, string> = {
  neutral: "bg-fg-subtle",
  accent: "bg-accent",
  sky: "bg-sky-500",
  blue: "bg-blue-500",
  indigo: "bg-indigo-500",
  violet: "bg-violet-500",
  green: "bg-emerald-500",
  amber: "bg-amber-500",
  orange: "bg-orange-500",
  red: "bg-rose-500",
};

// ---------- buttons ----------

type ButtonVariant = "default" | "primary" | "danger" | "ghost" | "success" | "soft";
type ButtonSize = "xs" | "sm" | "md" | "lg";

const BUTTON_SIZE: Record<ButtonSize, string> = {
  xs: "h-6 text-[11px] gap-1 rounded-md",
  sm: "h-7 text-xs gap-1.5 rounded-md",
  md: "h-9 text-sm gap-2 rounded-lg",
  lg: "h-11 text-[15px] gap-2 rounded-xl",
};
// Padding lives apart from size so icon-only buttons never carry two conflicting px-* classes.
const BUTTON_PAD: Record<ButtonSize, string> = { xs: "px-2", sm: "px-2.5", md: "px-3.5", lg: "px-5" };
const ICON_SIZE: Record<ButtonSize, string> = { xs: "w-6", sm: "w-7", md: "w-9", lg: "w-11" };

const BUTTON_VARIANT: Record<ButtonVariant, string> = {
  default: "border border-line bg-surface text-fg shadow-card hover:border-line-strong hover:bg-surface-2",
  primary: "bg-accent text-white shadow-card inset-shadow-[inset_0_1px_0_rgb(255_255_255/0.16)] hover:bg-accent-hover",
  success: "bg-emerald-600 text-white shadow-card inset-shadow-[inset_0_1px_0_rgb(255_255_255/0.16)] hover:bg-emerald-700 dark:hover:bg-emerald-500",
  danger: "bg-rose-600 text-white shadow-card inset-shadow-[inset_0_1px_0_rgb(255_255_255/0.16)] hover:bg-rose-700 dark:hover:bg-rose-500",
  ghost: "text-fg-muted hover:bg-surface-2 hover:text-fg",
  soft: "bg-accent-soft text-accent-fg hover:bg-accent/20",
};

export function Button({
  variant = "default",
  size = "md",
  icon,
  className,
  ...props
}: ButtonHTMLAttributes<HTMLButtonElement> & { variant?: ButtonVariant; size?: ButtonSize; icon?: boolean }) {
  return (
    <button
      {...props}
      className={clsx(
        "inline-flex shrink-0 select-none items-center font-medium whitespace-nowrap transition-[background-color,border-color,color,box-shadow,transform] duration-150 enabled:active:scale-[0.97] disabled:cursor-not-allowed disabled:opacity-50",
        BUTTON_SIZE[size],
        icon ? clsx(ICON_SIZE[size], "justify-center") : BUTTON_PAD[size],
        BUTTON_VARIANT[variant],
        className,
      )}
    />
  );
}

// ---------- pills & badges ----------

export function Pill({ tone = "neutral", className, children, title }: { tone?: Tone; className?: string; children: ReactNode; title?: string }) {
  return (
    <span title={title} className={clsx("inline-flex h-5 items-center gap-1 rounded-full px-2 text-[11px] font-medium leading-none whitespace-nowrap", TONE[tone], className)}>
      {children}
    </span>
  );
}

export const STATE_TONE: Record<IssueState, Tone> = {
  triage: "violet",
  ready: "accent",
  changes_requested: "orange",
  merge_conflict: "red",
  ready_to_merge: "green",
  closed: "neutral",
  backlog: "neutral",
  in_progress: "sky",
  in_review: "indigo",
  done: "green",
};

export function StateBadge({ state, always }: { state: IssueState; always?: boolean }) {
  // On the board, the column already says "In progress"/"In review"/"Backlog"/"Done".
  if (!always && ["backlog", "in_progress", "in_review", "done"].includes(state)) return null;
  return (
    <Pill tone={STATE_TONE[state]}>
      <span className={clsx("h-1.5 w-1.5 rounded-full", TONE_DOT[STATE_TONE[state]])} />
      {STATE_LABEL[state]}
    </Pill>
  );
}

const HOLD_TONE: Record<Hold, Tone> = { needs_decision: "amber", stalled: "red", paused: "neutral" };
const HOLD_ICON: Record<Hold, typeof Pause> = { needs_decision: CircleHelp, stalled: AlertTriangle, paused: Pause };

export function HoldBadge({ hold, reason }: { hold: Hold; reason?: string | null }) {
  const Icon = HOLD_ICON[hold];
  return (
    <Pill tone={HOLD_TONE[hold]} title={reason ?? undefined} className={hold === "needs_decision" ? "ring-1 ring-amber-500/30" : undefined}>
      <Icon size={11} strokeWidth={2.4} /> {HOLD_LABEL[hold]}
    </Pill>
  );
}

export function LabelChip({ name, color }: { name: string; color: string }) {
  const c = `#${color.replace("#", "")}`;
  return (
    <span className="inline-flex h-5 items-center gap-1.5 rounded-full border border-line bg-surface px-2 text-[11px] font-medium leading-none whitespace-nowrap text-fg-muted">
      <span className="h-2 w-2 shrink-0 rounded-full" style={{ background: c }} />
      {name}
    </span>
  );
}

/** A small pulsing dot for "something is happening right now". */
export function LiveDot({ tone = "sky", className }: { tone?: Tone; className?: string }) {
  return (
    <span className={clsx("relative inline-flex h-2 w-2 shrink-0", className)}>
      <span className={clsx("absolute inline-flex h-full w-full animate-ping rounded-full opacity-60", TONE_DOT[tone])} />
      <span className={clsx("relative inline-flex h-2 w-2 rounded-full", TONE_DOT[tone])} />
    </span>
  );
}

// ---------- surfaces & layout ----------

export function Card({ className, children, ...props }: HTMLAttributes<HTMLDivElement>) {
  return (
    <div {...props} className={clsx("rounded-xl border border-line bg-surface shadow-card", className)}>
      {children}
    </div>
  );
}

/** A titled card: settings groups, lists, panels. `flush` drops the body padding for edge-to-edge rows. */
export function Section({
  title,
  description,
  actions,
  children,
  flush,
  className,
}: {
  title: ReactNode;
  description?: ReactNode;
  actions?: ReactNode;
  children: ReactNode;
  flush?: boolean;
  className?: string;
}) {
  return (
    <section className={clsx("overflow-hidden rounded-xl border border-line bg-surface shadow-card", className)}>
      <header className="flex flex-wrap items-center gap-x-3 gap-y-2 border-b border-line px-4 py-3 sm:px-5">
        <div className="min-w-0 flex-1">
          <h2 className="text-sm font-semibold tracking-tight">{title}</h2>
          {description && <p className="mt-0.5 text-xs leading-relaxed text-fg-muted">{description}</p>}
        </div>
        {actions && <div className="flex flex-wrap items-center gap-2">{actions}</div>}
      </header>
      <div className={clsx(!flush && "p-4 sm:p-5")}>{children}</div>
    </section>
  );
}

const PAGE_WIDTH = { sm: "max-w-2xl", md: "max-w-3xl", lg: "max-w-5xl", xl: "max-w-6xl" };

/** Standard page gutters and max width for non-board pages. */
export function Page({ children, width = "lg", className }: { children: ReactNode; width?: keyof typeof PAGE_WIDTH; className?: string }) {
  return <div className={clsx("mx-auto w-full px-4 pt-5 pb-10 sm:px-6 sm:pt-8 lg:px-8", PAGE_WIDTH[width], className)}>{children}</div>;
}

export function PageHeader({ title, subtitle, actions, className }: { title: ReactNode; subtitle?: ReactNode; actions?: ReactNode; className?: string }) {
  return (
    <div className={clsx("mb-6 flex flex-wrap items-end gap-x-4 gap-y-3", className)}>
      <div className="min-w-0 flex-1 basis-72">
        <h1 className="text-xl font-semibold tracking-tight sm:text-2xl">{title}</h1>
        {subtitle && <p className="mt-1 text-sm text-fg-muted">{subtitle}</p>}
      </div>
      {actions && <div className="flex flex-wrap items-center gap-2">{actions}</div>}
    </div>
  );
}

export function EmptyState({ icon, title, children, action, className }: { icon?: ReactNode; title: ReactNode; children?: ReactNode; action?: ReactNode; className?: string }) {
  return (
    <div className={clsx("flex flex-col items-center justify-center px-6 py-10 text-center", className)}>
      {icon && <div className="mb-3 grid h-11 w-11 place-items-center rounded-2xl border border-line bg-surface-2 text-fg-subtle">{icon}</div>}
      <div className="text-sm font-medium">{title}</div>
      {children && <div className="mt-1 max-w-sm text-sm text-fg-muted">{children}</div>}
      {action && <div className="mt-4">{action}</div>}
    </div>
  );
}

// ---------- controls ----------

/** A pill-shaped single choice (filters, ranges, view modes). */
export function Segmented<T extends string | number | null>({
  options,
  value,
  onChange,
  className,
  size = "md",
  label,
}: {
  options: { value: T; label: ReactNode; count?: number }[];
  value: T;
  onChange: (v: T) => void;
  className?: string;
  size?: "sm" | "md";
  label?: string;
}) {
  return (
    <div role="group" aria-label={label} className={clsx("inline-flex max-w-full items-center gap-0.5 overflow-x-auto rounded-lg bg-surface-3/70 p-0.5 no-scrollbar", className)}>
      {options.map((o) => {
        const active = o.value === value;
        return (
          <button
            key={String(o.value)}
            type="button"
            aria-pressed={active}
            onClick={() => onChange(o.value)}
            className={clsx(
              "inline-flex shrink-0 items-center gap-1.5 rounded-md font-medium whitespace-nowrap transition-colors",
              size === "sm" ? "h-6 px-2 text-xs" : "h-7 px-3 text-[13px]",
              active ? "bg-surface text-fg shadow-card" : "text-fg-muted hover:text-fg",
            )}
          >
            {o.label}
            {o.count !== undefined && <span className={clsx("tabular-nums", active ? "text-fg-muted" : "text-fg-subtle")}>{o.count}</span>}
          </button>
        );
      })}
    </div>
  );
}

export function Switch({
  checked,
  onChange,
  label,
  disabled,
  tone = "accent",
  title,
  size = "md",
}: {
  checked: boolean;
  onChange: (v: boolean) => void;
  label: string;
  disabled?: boolean;
  tone?: "accent" | "green";
  title?: string;
  size?: "sm" | "md";
}) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      aria-label={label}
      title={title}
      disabled={disabled}
      onClick={() => onChange(!checked)}
      className={clsx(
        "relative inline-flex shrink-0 items-center rounded-full transition-colors duration-200 disabled:cursor-not-allowed disabled:opacity-50",
        size === "sm" ? "h-4 w-7" : "h-5 w-9",
        checked ? (tone === "green" ? "bg-emerald-500" : "bg-accent") : "bg-line-strong",
      )}
    >
      <span
        className={clsx(
          "absolute left-0.5 rounded-full bg-white shadow-[0_1px_2px_rgb(0_0_0/0.3)] transition-transform duration-200",
          size === "sm" ? "h-3 w-3" : "h-4 w-4",
          checked && (size === "sm" ? "translate-x-3" : "translate-x-4"),
        )}
      />
    </button>
  );
}

/** Underline tabs (page sections). */
export function Tabs<T extends string>({ tabs, value, onChange, className }: { tabs: { id: T; label: ReactNode; badge?: ReactNode }[]; value: T; onChange: (t: T) => void; className?: string }) {
  return (
    <div role="tablist" className={clsx("flex gap-1 overflow-x-auto border-b border-line no-scrollbar", className)}>
      {tabs.map((t) => {
        const active = t.id === value;
        return (
          <button
            key={t.id}
            role="tab"
            type="button"
            aria-selected={active}
            onClick={() => onChange(t.id)}
            className={clsx(
              "relative inline-flex h-10 shrink-0 items-center gap-1.5 px-3 text-sm font-medium transition-colors",
              active ? "text-fg" : "text-fg-muted hover:text-fg",
            )}
          >
            {t.label}
            {t.badge}
            {active && <span className="absolute inset-x-2 -bottom-px h-0.5 rounded-full bg-accent" />}
          </button>
        );
      })}
    </div>
  );
}

// ---------- identity ----------

const AVATAR_HUES = ["bg-rose-500", "bg-orange-500", "bg-amber-500", "bg-emerald-500", "bg-teal-500", "bg-sky-500", "bg-indigo-500", "bg-violet-500", "bg-fuchsia-500"];

function hash(s: string) {
  let h = 0;
  for (const ch of s) h = (h * 31 + ch.charCodeAt(0)) | 0;
  return Math.abs(h);
}

export function Avatar({ name, agent, size = 24, className }: { name?: string | null; agent?: boolean; size?: number; className?: string }) {
  const n = name ?? "?";
  if (agent) {
    return (
      <span
        className={clsx("grid shrink-0 place-items-center rounded-lg bg-gradient-to-br from-indigo-500 to-violet-600 text-white shadow-card", className)}
        style={{ width: size, height: size }}
        aria-hidden
      >
        <BotGlyph size={Math.round(size * 0.58)} />
      </span>
    );
  }
  return (
    <span
      className={clsx("grid shrink-0 place-items-center rounded-full font-semibold text-white uppercase", AVATAR_HUES[hash(n) % AVATAR_HUES.length], className)}
      style={{ width: size, height: size, fontSize: Math.round(size * 0.44) }}
      aria-hidden
    >
      {n.trim().charAt(0) || "?"}
    </span>
  );
}

function BotGlyph({ size }: { size: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={2.2} strokeLinecap="round" strokeLinejoin="round">
      <path d="M12 8V4H8" />
      <rect width="16" height="12" x="4" y="8" rx="3" />
      <path d="M9 13v2M15 13v2" />
    </svg>
  );
}

/** A project's initial on a colour derived from its name. */
export function ProjectMark({ name, size = 20 }: { name: string; size?: number }) {
  const grads = ["from-sky-500 to-indigo-500", "from-emerald-500 to-teal-600", "from-amber-500 to-orange-600", "from-fuchsia-500 to-violet-600", "from-rose-500 to-pink-600"];
  return (
    <span
      aria-hidden
      className={clsx("grid shrink-0 place-items-center rounded-md bg-gradient-to-br font-semibold text-white uppercase", grads[hash(name) % grads.length])}
      style={{ width: size, height: size, fontSize: Math.round(size * 0.5) }}
    >
      {name.charAt(0)}
    </span>
  );
}

export function Logo({ size = 22, className }: { size?: number; className?: string }) {
  // Unique per instance: a gradient defined inside a display:none copy (e.g. the hidden sidebar
  // on phones) paints nothing for every other copy that references the same id.
  const id = `akb-logo-${useId().replace(/[^\w-]/g, "")}`;
  return (
    <svg width={size} height={size} viewBox="0 0 32 32" className={clsx("shrink-0", className)} aria-hidden>
      <defs>
        <linearGradient id={id} x1="0" y1="0" x2="1" y2="1">
          <stop offset="0" stopColor="#8583f2" />
          <stop offset="1" stopColor="#4f47c7" />
        </linearGradient>
      </defs>
      <rect width="32" height="32" rx="8" fill={`url(#${id})`} />
      <rect x="7" y="8" width="5" height="16" rx="2" fill="#fff" />
      <rect x="13.5" y="8" width="5" height="11" rx="2" fill="#fff" fillOpacity=".85" />
      <rect x="20" y="8" width="5" height="7" rx="2" fill="#fff" fillOpacity=".7" />
    </svg>
  );
}

// ---------- markdown ----------

// Images nested inside a markdown link (`[![alt](img)](url)`) should stay
// non-interactive so the enclosing link remains the single keyboard target.
const InsideLinkContext = createContext(false);

// A stable component identity, so react-markdown doesn't remount every <a>
// (and any focused <img> inside it) each time Markdown re-renders.
const LinkedImage: Components["a"] = ({ href, title, children }) => (
  <a href={href} title={title}>
    <InsideLinkContext.Provider value={true}>{children}</InsideLinkContext.Provider>
  </a>
);

export function Markdown({ children, className }: { children: string; className?: string }) {
  const [zoomed, setZoomed] = useState<{ src: string; alt: string } | null>(null);
  const dialogRef = useRef<HTMLDivElement | null>(null);
  const closeButtonRef = useRef<HTMLButtonElement | null>(null);
  const previouslyFocused = useRef<HTMLElement | null>(null);
  const openZoomed = useCallback((src: string, alt: string) => setZoomed({ src, alt }), []);

  useEffect(() => {
    if (!zoomed) return;
    previouslyFocused.current = document.activeElement as HTMLElement | null;
    closeButtonRef.current?.focus();

    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        setZoomed(null);
        return;
      }
      if (e.key !== "Tab") return;
      const focusable = dialogRef.current?.querySelectorAll<HTMLElement>(FOCUSABLE_SELECTOR);
      if (!focusable || focusable.length === 0) return;
      const list = Array.from(focusable);
      const first = list[0];
      const last = list[list.length - 1];
      if (e.shiftKey && document.activeElement === first) {
        e.preventDefault();
        last.focus();
      } else if (!e.shiftKey && document.activeElement === last) {
        e.preventDefault();
        first.focus();
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => {
      window.removeEventListener("keydown", onKeyDown);
      previouslyFocused.current?.focus();
    };
  }, [zoomed]);

  const ZoomableImage = useCallback(
    ({ src, alt }: ComponentProps<"img"> & ExtraProps) => {
      // eslint-disable-next-line react-hooks/rules-of-hooks
      const insideLink = useContext(InsideLinkContext);
      if (typeof src !== "string") return null;
      if (insideLink) return <img src={src} alt={alt ?? ""} />;
      return (
        // eslint-disable-next-line jsx-a11y/no-noninteractive-element-interactions
        <img
          src={src}
          alt={alt ?? ""}
          role="button"
          tabIndex={0}
          className="cursor-zoom-in"
          onClick={() => openZoomed(src, alt ?? "")}
          onKeyDown={(e) => {
            if (e.key !== "Enter" && e.key !== " ") return;
            e.preventDefault();
            openZoomed(src, alt ?? "");
          }}
        />
      );
    },
    [openZoomed],
  );

  const components = useMemo<Components>(() => ({ a: LinkedImage, img: ZoomableImage }), [ZoomableImage]);

  return (
    <div className={clsx("prose-sm break-words", className)}>
      <ReactMarkdown remarkPlugins={[remarkGfm]} components={components}>
        {children}
      </ReactMarkdown>
      {zoomed && (
        <div
          ref={dialogRef}
          role="dialog"
          aria-modal="true"
          aria-label={zoomed.alt || "Full size image"}
          className="fixed inset-0 z-[60] flex animate-fade-in items-center justify-center bg-black/85 p-4 backdrop-blur-sm"
          onClick={() => setZoomed(null)}
        >
          <img src={zoomed.src} alt={zoomed.alt} className="max-h-[calc(100%_-_3rem)] max-w-full cursor-zoom-out rounded-lg object-contain shadow-overlay" />
          <div className="absolute top-[max(0.75rem,env(safe-area-inset-top))] right-3 flex items-center gap-2">
            <a
              href={zoomed.src}
              target="_blank"
              rel="noopener noreferrer"
              onClick={(e) => e.stopPropagation()}
              className="rounded-full bg-white/10 px-3 py-1.5 text-sm text-zinc-100 backdrop-blur hover:bg-white/20 hover:text-white"
            >
              Open original
            </a>
            <button
              ref={closeButtonRef}
              type="button"
              onClick={(e) => {
                e.stopPropagation();
                setZoomed(null);
              }}
              aria-label="Close"
              className="grid h-8 w-8 place-items-center rounded-full bg-white/10 text-zinc-100 backdrop-blur hover:bg-white/20 hover:text-white"
            >
              <X size={16} />
            </button>
          </div>
        </div>
      )}
    </div>
  );
}

// ---------- time ----------

export function timeAgo(iso?: string | null): string {
  if (!iso) return "";
  const s = (Date.now() - new Date(iso).getTime()) / 1000;
  if (s < -60) return `in ${fmtDur(-s)}`;
  if (s < 60) return "just now";
  return `${fmtDur(s)} ago`;
}

export function fmtDur(s: number): string {
  if (s < 3600) return `${Math.max(1, Math.round(s / 60))}m`;
  if (s < 86400) return `${Math.floor(s / 3600)}h ${Math.floor((s % 3600) / 60)}m`;
  return `${Math.floor(s / 86400)}d`;
}

export function TimeAgo({ iso, className }: { iso?: string | null; className?: string }) {
  const [, tick] = useState(0);
  useEffect(() => {
    const t = setInterval(() => tick((x) => x + 1), 30000);
    return () => clearInterval(t);
  }, []);
  if (!iso) return null;
  return (
    <time dateTime={iso} title={new Date(iso).toLocaleString()} className={className}>
      {timeAgo(iso)}
    </time>
  );
}

// ---------- feedback ----------

export function ErrorBox({ error, className }: { error: unknown; className?: string }) {
  if (!error) return null;
  return (
    <div className={clsx("flex items-start gap-2 rounded-lg border border-rose-500/25 bg-rose-500/8 px-3 py-2 text-sm break-words text-rose-700 dark:text-rose-300", className)}>
      <AlertCircle size={15} className="mt-0.5 shrink-0" />
      <div className="min-w-0 flex-1">{error instanceof Error ? error.message : String(error)}</div>
    </div>
  );
}

export function Spinner({ className }: { className?: string }) {
  return <span className={clsx("inline-block h-3 w-3 animate-spin rounded-full border-2 border-current border-t-transparent", className)} />;
}

// ---------- dialogs ----------

export const FOCUSABLE_SELECTOR = 'a[href], button:not([disabled]), select:not([disabled]), input:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';

/** A centred dialog on wide screens, and a bottom sheet on phones. */
export function Modal({ open, onClose, title, children, wide }: { open: boolean; onClose: () => void; title: ReactNode; children: ReactNode; wide?: boolean }) {
  const panelRef = useRef<HTMLDivElement>(null);
  const previouslyFocused = useRef<HTMLElement | null>(null);
  // Move focus into the dialog when it opens, and restore it when it closes, so it never lingers
  // on (or returns to) a trigger that's now obscured by the backdrop.
  useEffect(() => {
    const panel = panelRef.current;
    if (open) {
      previouslyFocused.current = document.activeElement instanceof HTMLElement ? document.activeElement : null;
      const focusable = panel ? Array.from(panel.querySelectorAll<HTMLElement>(FOCUSABLE_SELECTOR)).filter((el) => el.offsetParent !== null) : [];
      (focusable[0] ?? panel)?.focus();
    } else if (previouslyFocused.current?.isConnected) {
      previouslyFocused.current.focus();
      previouslyFocused.current = null;
    }
  }, [open]);
  useEffect(() => {
    if (!open) return;
    const k = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        onClose();
        return;
      }
      if (e.key !== "Tab" || !panelRef.current) return;
      // Trap Tab/Shift+Tab inside the open dialog so it can't escape into the obscured page behind it.
      const focusable = Array.from(panelRef.current.querySelectorAll<HTMLElement>(FOCUSABLE_SELECTOR)).filter((el) => el.offsetParent !== null);
      if (focusable.length === 0) return;
      const [firstEl, lastEl] = [focusable[0], focusable[focusable.length - 1]];
      if (!panelRef.current.contains(document.activeElement)) {
        e.preventDefault();
        (e.shiftKey ? lastEl : firstEl).focus();
        return;
      }
      if (e.shiftKey && document.activeElement === firstEl) {
        e.preventDefault();
        lastEl.focus();
      } else if (!e.shiftKey && document.activeElement === lastEl) {
        e.preventDefault();
        firstEl.focus();
      }
    };
    window.addEventListener("keydown", k);
    return () => window.removeEventListener("keydown", k);
  }, [open, onClose]);
  if (!open) return null;
  return (
    <div className="fixed inset-0 z-50 flex items-end justify-center sm:items-start sm:p-4 sm:pt-[10vh]" onMouseDown={onClose}>
      <div className="absolute inset-0 animate-fade-in bg-black/40 backdrop-blur-[2px]" />
      <div
        ref={panelRef}
        tabIndex={-1}
        role="dialog"
        aria-modal="true"
        aria-label={typeof title === "string" ? title : undefined}
        className={clsx(
          "relative flex max-h-[92dvh] w-full animate-sheet-up flex-col overflow-hidden rounded-t-2xl border border-line bg-surface shadow-overlay outline-none sm:max-h-[80vh] sm:animate-pop-in sm:rounded-2xl",
          wide ? "sm:max-w-3xl" : "sm:max-w-lg",
        )}
        onMouseDown={(e) => e.stopPropagation()}
      >
        <div className="mx-auto mt-2 h-1 w-9 shrink-0 rounded-full bg-line-strong sm:hidden" aria-hidden />
        <div className="flex shrink-0 items-center justify-between gap-3 border-b border-line px-5 py-3.5">
          <h2 className="min-w-0 text-[15px] font-semibold tracking-tight">{title}</h2>
          <button onClick={onClose} className="-mr-1.5 grid h-8 w-8 shrink-0 place-items-center rounded-lg text-fg-subtle hover:bg-surface-2 hover:text-fg" aria-label="Close dialog">
            <X size={17} />
          </button>
        </div>
        <div className="min-h-0 flex-1 overflow-y-auto p-5 pb-[max(1.25rem,env(safe-area-inset-bottom))]">{children}</div>
      </div>
    </div>
  );
}

// ---------- forms ----------

export function Field({ label, children, hint, className }: { label: string; children: ReactNode; hint?: ReactNode; className?: string }) {
  return (
    <label className={clsx("block min-w-0 space-y-1.5", className)}>
      <span className="block text-xs font-medium text-fg-muted">{label}</span>
      {children}
      {hint && <span className="block text-[11px] leading-relaxed text-fg-subtle">{hint}</span>}
    </label>
  );
}

// Phones get 16px text in every field so iOS Safari doesn't zoom in on focus.
const fieldBase =
  "rounded-lg border border-line bg-surface text-fg shadow-card outline-none placeholder:text-fg-subtle transition-[border-color,box-shadow] focus:border-accent/60 focus:ring-3 focus:ring-accent/15 disabled:cursor-not-allowed disabled:opacity-60";

/** Inline-width field (selects and inputs that sit in a row). */
export const fieldCls = clsx(fieldBase, "px-3 py-1.5 text-base sm:text-sm");
/** Full-width field. */
export const inputCls = clsx(fieldBase, "w-full px-3 py-2 text-base sm:text-sm");
/** Compact inline field for dense rows. */
export const fieldSmCls = clsx(fieldBase, "px-2.5 py-1 text-base sm:text-xs");
/** Add to a <select> for the themed chevron. */
export const selectCls = "ak-select";

export function short(sha?: string | null) {
  return sha ? sha.slice(0, 8) : "";
}
