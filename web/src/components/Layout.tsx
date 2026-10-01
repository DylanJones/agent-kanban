import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import clsx from "clsx";
import {
  Activity,
  BarChart3,
  BookOpen,
  Bot,
  Check,
  CheckCircle2,
  ChevronsUpDown,
  CircleSlash,
  Inbox,
  KanbanSquare,
  Menu,
  PauseCircle,
  Plus,
  RefreshCw,
  Settings2,
  SunMoon,
  X,
  XCircle,
} from "lucide-react";
import { type ReactNode, useCallback, useEffect, useId, useRef, useState } from "react";
import { Link, NavLink, useLocation, useNavigate, useParams } from "react-router";
import { ApiError, ROLE_LABEL, type RunView, api, client, unwrap } from "../api/client";
import { useLiveEvents } from "../api/live";
import { useThemePreference } from "../theme";
import { BrowserSettingsModal } from "./BrowserSettings";
import { ConnectClaudeModal } from "./ConnectClaude";
import { Button, FOCUSABLE_SELECTOR, LiveDot, Logo, ProjectMark, Switch, inputCls } from "./ui";

export function useProjects() {
  return useQuery({ queryKey: ["projects"], queryFn: () => unwrap(client.GET("/api/projects")) });
}

export function useSettings() {
  return useQuery({ queryKey: ["settings"], queryFn: () => unwrap(client.GET("/api/settings")) });
}

export function useMe() {
  return useQuery({ queryKey: ["me"], queryFn: () => unwrap(client.GET("/api/me")) });
}

function useActiveRuns() {
  return useQuery({
    queryKey: ["runs", "active"],
    queryFn: () => unwrap(client.GET("/api/runs", { params: { query: { status: "active" } } })),
    refetchInterval: 15000,
  });
}

export function useInboxCount(slug?: string) {
  return useQuery({
    queryKey: ["inbox", "count", slug],
    enabled: !!slug,
    queryFn: async () => {
      const [holds, perms, board] = await Promise.all([
        unwrap(client.GET("/api/projects/{p}/issues", { params: { path: { p: slug! }, query: { hold: "any" } } })),
        unwrap(client.GET("/api/permission-requests", { params: { query: {} } })),
        unwrap(client.GET("/api/projects/{p}/board", { params: { path: { p: slug! }, query: { badge: "ready_to_merge" } } })),
      ]);
      const rtm = board.columns.reduce((n, c) => n + c.cards.length, 0);
      return holds.length + perms.length + rtm;
    },
  });
}

/** Small: whether the running server is behind the checkout's HEAD, for the "Server" nav item. */
function useCommitsBehind(enabled: boolean) {
  const status = useQuery({
    queryKey: ["build-status"],
    queryFn: () => unwrap(client.GET("/api/build-status")),
    enabled,
    refetchInterval: 60000,
  });
  return status.data?.commits_behind ?? undefined;
}

// ---------- navigation model ----------

type NavEntry = { to: string; label: string; icon: typeof Inbox; badge?: number; live?: boolean; end?: boolean };

function useNavEntries(current?: string, inbox?: number, isHuman?: boolean) {
  const behind = useCommitsBehind(!!isHuman);
  const active = useActiveRuns().data?.length ?? 0;
  const primary: NavEntry[] = [
    ...(current
      ? [
          { to: `/p/${current}`, label: "Board", icon: KanbanSquare, end: true },
          { to: `/p/${current}/inbox`, label: "Inbox", icon: Inbox, badge: inbox || undefined },
        ]
      : []),
    { to: "/runs", label: "Runs", icon: Activity, badge: active || undefined, live: active > 0 },
  ];
  const secondary: NavEntry[] = [
    { to: "/agents", label: "Agents", icon: Bot },
    { to: "/usage", label: "Usage", icon: BarChart3 },
    ...(current ? [{ to: `/p/${current}/settings`, label: "Project", icon: Settings2 }] : []),
    ...(isHuman ? [{ to: "/server", label: "Server", icon: RefreshCw, badge: behind || undefined }] : []),
  ];
  return { primary, secondary };
}

function Badge({ n, live, className }: { n: number; live?: boolean; className?: string }) {
  return (
    <span
      className={clsx(
        "inline-flex h-[18px] min-w-[18px] items-center justify-center gap-1 rounded-full px-1.5 text-[10px] font-semibold tabular-nums",
        live ? "bg-sky-500/15 text-sky-700 dark:text-sky-300" : "bg-amber-500 text-white",
        className,
      )}
    >
      {live && <span className="h-1.5 w-1.5 animate-pulse rounded-full bg-sky-500" />}
      {n}
    </span>
  );
}

function SideNavItem({ e }: { e: NavEntry }) {
  const Icon = e.icon;
  return (
    <NavLink
      to={e.to}
      end={e.end}
      className={({ isActive }) =>
        clsx(
          "group flex h-8 items-center gap-2.5 rounded-lg px-2.5 text-[13px] font-medium transition-colors",
          isActive ? "bg-surface text-fg shadow-card ring-1 ring-line" : "text-fg-muted hover:bg-surface-2 hover:text-fg",
        )
      }
    >
      {({ isActive }) => (
        <>
          <Icon size={16} className={clsx("shrink-0", isActive ? "text-accent" : "text-fg-subtle group-hover:text-fg-muted")} />
          <span className="flex-1 truncate">{e.label}</span>
          {!!e.badge && <Badge n={e.badge} live={e.live} />}
        </>
      )}
    </NavLink>
  );
}

// ---------- project switcher ----------

/** The current project, and a menu to switch to another one (or add one). */
function ProjectSwitcher({ current, className, compact }: { current?: string; className?: string; compact?: boolean }) {
  const projects = useProjects();
  const nav = useNavigate();
  const [open, setOpen] = useState(false);
  const root = useRef<HTMLDivElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const menu = useRef<HTMLDivElement>(null);
  const menuId = useId();
  const list = projects.data ?? [];
  const p = list.find((x) => x.slug === current) ?? list[0];
  // Close when a press lands anywhere outside; on opening, focus the current project.
  useEffect(() => {
    if (!open) return;
    const items = menu.current?.querySelectorAll<HTMLElement>("[role^=menuitem]");
    (menu.current?.querySelector<HTMLElement>("[aria-checked=true]") ?? items?.[0])?.focus();
    const down = (e: PointerEvent) => {
      if (!root.current?.contains(e.target as Node)) setOpen(false);
    };
    document.addEventListener("pointerdown", down);
    return () => document.removeEventListener("pointerdown", down);
  }, [open]);
  if (!p) return null;
  const close = () => {
    setOpen(false);
    trigger.current?.focus();
  };
  const onMenuKey = (e: React.KeyboardEvent) => {
    const items = Array.from(menu.current?.querySelectorAll<HTMLElement>("[role^=menuitem]") ?? []);
    const i = items.indexOf(document.activeElement as HTMLElement);
    const go = (n: number) => {
      e.preventDefault();
      items[(n + items.length) % items.length]?.focus();
    };
    if (e.key === "ArrowDown") go(i + 1);
    else if (e.key === "ArrowUp") go(i - 1);
    else if (e.key === "Home") go(0);
    else if (e.key === "End") go(items.length - 1);
    else if (e.key === "Escape") {
      // Only the menu closes, not the sheet or dialog it sits in.
      e.preventDefault();
      e.stopPropagation();
      close();
    } else if (e.key === "Tab") setOpen(false);
  };
  const item = "flex w-full items-center gap-2.5 rounded-lg px-2.5 py-2 text-left text-sm outline-none transition-colors focus:bg-surface-2";
  return (
    <div ref={root} className={clsx("relative min-w-0", className)}>
      <button
        ref={trigger}
        type="button"
        aria-haspopup="menu"
        aria-expanded={open}
        aria-controls={open ? menuId : undefined}
        title="Switch project"
        onClick={() => setOpen((o) => !o)}
        onKeyDown={(e) => {
          if (e.key === "ArrowDown" && !open) {
            e.preventDefault();
            setOpen(true);
          }
        }}
        className={clsx(
          "flex w-full min-w-0 items-center gap-2 rounded-lg text-left transition-colors hover:bg-surface-2",
          compact ? "h-8 px-1.5" : "h-10 border border-line bg-surface px-2.5 shadow-card",
          open && (compact ? "bg-surface-2" : "border-line-strong bg-surface-2"),
        )}
      >
        <ProjectMark name={p.name} size={compact ? 20 : 22} />
        <span className="min-w-0 flex-1 truncate text-sm font-semibold tracking-tight">{p.name}</span>
        <ChevronsUpDown size={14} className="shrink-0 text-fg-subtle" />
      </button>
      {open && (
        <div
          ref={menu}
          id={menuId}
          role="menu"
          aria-label="Projects"
          onKeyDown={onMenuKey}
          className={clsx(
            "absolute top-full z-50 mt-1.5 max-w-[calc(100vw-1.5rem)] origin-top animate-pop-in rounded-xl border border-line bg-surface p-1 shadow-overlay",
            compact ? "left-0 w-72" : "inset-x-0",
          )}
        >
          <div className="px-2.5 pt-1.5 pb-1 text-[11px] font-medium text-fg-subtle">Projects</div>
          {list.map((x) => {
            const selected = x.slug === p.slug;
            return (
              <button
                key={x.slug}
                type="button"
                role="menuitemradio"
                aria-checked={selected}
                tabIndex={-1}
                onMouseMove={(e) => e.currentTarget.focus()}
                onClick={() => {
                  close();
                  if (!selected) nav(`/p/${x.slug}`);
                }}
                className={item}
              >
                <ProjectMark name={x.name} size={24} />
                <span className="min-w-0 flex-1">
                  <span className="block truncate font-medium">{x.name}</span>
                  <span className="block truncate text-[11px] text-fg-subtle">{x.github_repo || x.slug}</span>
                </span>
                {selected && <Check size={15} strokeWidth={2.5} className="shrink-0 text-accent" />}
              </button>
            );
          })}
          <div className="mx-1 my-1 h-px bg-line" />
          <Link to="/new-project" role="menuitem" tabIndex={-1} onMouseMove={(e) => e.currentTarget.focus()} onClick={() => setOpen(false)} className={clsx(item, "text-fg-muted")}>
            <span className="grid h-6 w-6 shrink-0 place-items-center rounded-md border border-dashed border-line-strong">
              <Plus size={13} />
            </span>
            Add project
          </Link>
        </div>
      )}
    </div>
  );
}

// ---------- scheduler ----------

function SchedulerControl() {
  const qc = useQueryClient();
  const settings = useSettings();
  const runs = useActiveRuns();
  const toggle = useMutation({
    mutationFn: (enabled: boolean) => unwrap(client.PATCH("/api/settings", { body: { scheduler_enabled: enabled } })),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["settings"] }),
  });
  if (!settings.data) return null;
  const on = settings.data.scheduler_enabled;
  const active = runs.data?.length ?? 0;
  const max = settings.data.max_concurrent_runs;
  return (
    <div className="rounded-xl border border-line bg-surface p-3 shadow-card">
      <div className="flex items-center gap-2.5">
        <div className={clsx("grid h-8 w-8 shrink-0 place-items-center rounded-lg", on ? "bg-emerald-500/12 text-emerald-600 dark:text-emerald-400" : "bg-surface-3 text-fg-subtle")}>
          <Bot size={16} />
        </div>
        <div className="min-w-0 flex-1">
          <div className="text-[13px] font-medium">Scheduler {on ? "on" : "off"}</div>
          <div className="text-[11px] text-fg-muted tabular-nums" title="Active agent runs / concurrency limit">
            {active} of {max} running
          </div>
        </div>
        <Switch
          checked={on}
          onChange={(v) => toggle.mutate(v)}
          label="Scheduler"
          tone="green"
          title={on ? "Agents are being dispatched automatically. Switch off to stop dispatching new runs." : "Automatic dispatch is off. Switch on to start."}
        />
      </div>
      {max > 0 && max <= 16 && (
        <div className="mt-2.5 flex gap-1" aria-hidden>
          {Array.from({ length: max }, (_, i) => (
            <span key={i} className={clsx("h-1 flex-1 rounded-full", i < active ? "bg-sky-500" : "bg-surface-3")} />
          ))}
        </div>
      )}
    </div>
  );
}

/** Phones: running count and whether the scheduler is on, at a glance (the switch is in the menu). */
function SchedulerChip({ onClick }: { onClick: () => void }) {
  const settings = useSettings();
  const runs = useActiveRuns();
  if (!settings.data) return null;
  const on = settings.data.scheduler_enabled;
  const active = runs.data?.length ?? 0;
  return (
    <button
      onClick={onClick}
      className={clsx(
        "flex h-7 items-center gap-1.5 rounded-full px-2.5 text-xs font-medium tabular-nums",
        on ? "bg-emerald-500/12 text-emerald-700 dark:text-emerald-300" : "bg-surface-3 text-fg-muted",
      )}
      title={on ? "Scheduler on" : "Scheduler off"}
      aria-label={`Scheduler ${on ? "on" : "off"}, ${active} of ${settings.data.max_concurrent_runs} agents running`}
    >
      {active > 0 ? <LiveDot tone={on ? "green" : "sky"} /> : <Bot size={13} />}
      {active}/{settings.data.max_concurrent_runs}
    </button>
  );
}

// ---------- run toasts ----------

const TOAST_ICON: Record<string, [typeof CheckCircle2, string]> = {
  succeeded: [CheckCircle2, "text-emerald-500"],
  failed: [XCircle, "text-rose-500"],
  rate_limited: [PauseCircle, "text-amber-500"],
};

/** How many run toasts show at once; any beyond that collapse into a "+N more" link to the runs page. */
export const MAX_TOASTS = 3;
const TOAST_MS = 10000;

/** Pops a toast when an agent run finishes, and shows the running count in the tab title. */
export function RunToasts() {
  const runs = useActiveRuns();
  const prev = useRef<Set<number> | null>(null);
  const [toasts, setToasts] = useState<RunView[]>([]);
  const dismiss = useCallback((id: number) => setToasts((t) => t.filter((x) => x.id !== id)), []);
  useEffect(() => {
    if (!runs.data) return;
    const now = new Set(runs.data.map((r) => r.id));
    const gone = prev.current ? [...prev.current].filter((id) => !now.has(id)) : [];
    prev.current = now;
    document.title = now.size ? `(${now.size} running) agent-kanban` : "agent-kanban";
    for (const id of gone) {
      unwrap(client.GET("/api/runs/{id}", { params: { path: { id } } })).then((r) => {
        setToasts((t) => [...t, r]);
        setTimeout(() => dismiss(r.id), TOAST_MS);
        if (document.hidden && "Notification" in window && Notification.permission === "granted") {
          new Notification(`${ROLE_LABEL[r.role]} run ${r.status.replace("_", " ")}`, { body: `#${r.issue} ${r.issue_title ?? ""}` });
        }
      });
    }
  }, [runs.data, dismiss]);
  useEffect(() => {
    if ("Notification" in window && Notification.permission === "default") {
      const ask = () => Notification.requestPermission();
      window.addEventListener("click", ask, { once: true });
      return () => window.removeEventListener("click", ask);
    }
  }, []);
  const shown = toasts.slice(-MAX_TOASTS);
  const more = toasts.length - shown.length;
  // Phones: a short stack under the status bar, clear of the bottom tab bar. Desktop: bottom-right.
  // z-40 keeps it above the issue drawer (rendered earlier) but under the menu drawer and dialogs (z-50),
  // so it never covers or intercepts their controls.
  return (
    <div
      role="status"
      aria-label="Run notifications"
      className="pointer-events-none fixed inset-x-0 top-[calc(0.5rem+env(safe-area-inset-top))] z-40 flex flex-col items-center gap-2 px-3 lg:inset-x-auto lg:top-auto lg:right-5 lg:bottom-5 lg:items-end"
    >
      {more > 0 && (
        <Link
          to="/runs"
          onClick={() => setToasts([])}
          className="pointer-events-auto animate-pop-in rounded-full border border-line bg-surface/95 px-3 py-1 text-xs font-medium text-fg-muted shadow-overlay backdrop-blur-xl hover:text-fg"
        >
          +{more} more {more === 1 ? "run" : "runs"} finished
        </Link>
      )}
      {shown.map((r) => {
        const [Icon, color] = TOAST_ICON[r.status] ?? [CircleSlash, "text-fg-subtle"];
        return (
          <div
            key={r.id}
            className="pointer-events-auto flex w-full max-w-sm animate-pop-in items-start rounded-2xl border border-line bg-surface/95 shadow-overlay backdrop-blur-xl transition-transform hover:-translate-y-0.5"
          >
            <Link to={`/runs/${r.id}`} onClick={() => dismiss(r.id)} className="flex min-w-0 flex-1 items-start gap-3 p-3 pr-1">
              <Icon size={20} className={clsx("mt-0.5 shrink-0", color)} />
              <div className="min-w-0 flex-1">
                <div className="text-sm font-medium">
                  {ROLE_LABEL[r.role]} run {r.status.replace("_", " ")}
                </div>
                <div className="truncate text-xs text-fg-muted">
                  #{r.issue} {r.issue_title} · {r.agent}
                </div>
                {r.error && <div className="mt-0.5 truncate text-xs text-rose-600 dark:text-rose-400">{r.error}</div>}
              </div>
            </Link>
            <button
              onClick={() => dismiss(r.id)}
              className="m-1.5 grid h-8 w-8 shrink-0 place-items-center rounded-full text-fg-subtle hover:bg-surface-2 hover:text-fg"
              aria-label={`Dismiss run #${r.id} notification`}
            >
              <X size={15} />
            </button>
          </div>
        );
      })}
    </div>
  );
}

function ConnectionDot({ connected }: { connected: boolean }) {
  return (
    <span
      title={connected ? "Live updates connected" : "Reconnecting…"}
      aria-label={connected ? "Live updates connected" : "Reconnecting"}
      role="status"
      className={clsx("h-2 w-2 shrink-0 rounded-full", connected ? "bg-emerald-500 shadow-[0_0_0_3px_rgb(16_185_129/0.15)]" : "animate-pulse bg-fg-subtle")}
    />
  );
}

// ---------- desktop sidebar ----------

function Sidebar({ current, inbox, isHuman, connected, onOpenSettings }: { current?: string; inbox?: number; isHuman?: boolean; connected: boolean; onOpenSettings: () => void }) {
  const { primary, secondary } = useNavEntries(current, inbox, isHuman);
  return (
    <aside className="hidden w-60 shrink-0 flex-col px-3 py-3 lg:flex">
      <div className="flex h-9 items-center gap-2 px-1.5">
        <Link to={current ? `/p/${current}` : "/"} className="flex min-w-0 flex-1 items-center gap-2 text-[15px] font-semibold tracking-tight">
          <Logo size={22} />
          agent-kanban
        </Link>
        <ConnectionDot connected={connected} />
      </div>
      <ProjectSwitcher current={current} className="mt-3" />
      <nav className="mt-4 flex flex-col gap-0.5">
        {primary.map((e) => (
          <SideNavItem key={e.to} e={e} />
        ))}
      </nav>
      <div className="mt-5 mb-1.5 px-2.5 text-[11px] font-medium tracking-wide text-fg-subtle">Configure</div>
      <nav className="flex flex-col gap-0.5">
        {secondary.map((e) => (
          <SideNavItem key={e.to} e={e} />
        ))}
      </nav>
      <div className="mt-auto space-y-2 pt-4">
        <SchedulerControl />
        <div className="flex items-center gap-1 px-1">
          <a href="/api/docs" target="_blank" className="flex h-7 items-center gap-1.5 rounded-md px-1.5 text-xs text-fg-muted hover:bg-surface-2 hover:text-fg">
            <BookOpen size={13} /> API docs
          </a>
          <button onClick={onOpenSettings} className="ml-auto grid h-7 w-7 place-items-center rounded-md text-fg-muted hover:bg-surface-2 hover:text-fg" aria-label="Browser settings" title="Appearance">
            <SunMoon size={15} />
          </button>
        </div>
      </div>
    </aside>
  );
}

// ---------- phone chrome ----------

function TabBar({ current, inbox, onMore, moreActive }: { current?: string; inbox?: number; onMore: () => void; moreActive: boolean }) {
  const active = useActiveRuns().data?.length ?? 0;
  const tabs: NavEntry[] = current
    ? [
        { to: `/p/${current}`, label: "Board", icon: KanbanSquare, end: true },
        { to: `/p/${current}/inbox`, label: "Inbox", icon: Inbox, badge: inbox || undefined },
        { to: "/runs", label: "Runs", icon: Activity, badge: active || undefined, live: active > 0 },
      ]
    : [
        { to: "/runs", label: "Runs", icon: Activity, badge: active || undefined, live: active > 0 },
        { to: "/agents", label: "Agents", icon: Bot },
        { to: "/usage", label: "Usage", icon: BarChart3 },
      ];
  const item = "relative flex flex-col items-center justify-center gap-0.5 text-[10px] font-medium transition-colors";
  return (
    <nav className="shrink-0 border-t border-line bg-surface/90 pb-[env(safe-area-inset-bottom)] backdrop-blur-xl lg:hidden" aria-label="Primary">
      <div className="grid h-14 grid-cols-4">
        {tabs.map((t) => (
          <NavLink key={t.to} to={t.to} end={t.end} className={({ isActive }) => clsx(item, isActive ? "text-accent" : "text-fg-subtle")}>
            <span className="relative">
              <t.icon size={22} strokeWidth={1.9} />
              {!!t.badge && (
                <span
                  className={clsx(
                    "absolute -top-1 -right-2.5 min-w-4 rounded-full px-1 text-center text-[9px] leading-4 font-semibold text-white tabular-nums ring-2 ring-surface",
                    t.live ? "bg-sky-500" : "bg-amber-500",
                  )}
                >
                  {t.badge}
                </span>
              )}
            </span>
            {t.label}
          </NavLink>
        ))}
        <button onClick={onMore} className={clsx(item, moreActive ? "text-accent" : "text-fg-subtle")} aria-label="Menu">
          <Menu size={22} strokeWidth={1.9} />
          More
        </button>
      </div>
    </nav>
  );
}

function TopBar({ current, connected, onMenu }: { current?: string; connected: boolean; onMenu: () => void }) {
  return (
    <header className="z-30 shrink-0 border-b border-line bg-surface/85 pt-[env(safe-area-inset-top)] backdrop-blur-xl lg:hidden">
      <div className="flex h-12 items-center gap-1.5 px-3">
        <Link to={current ? `/p/${current}` : "/"} aria-label="agent-kanban home" className="shrink-0">
          <Logo size={24} />
        </Link>
        <ProjectSwitcher current={current} compact className="max-w-[55vw]" />
        <div className="ml-auto flex items-center gap-2.5">
          <SchedulerChip onClick={onMenu} />
          <ConnectionDot connected={connected} />
        </div>
      </div>
    </header>
  );
}

/** Small screens: everything that isn't in the tab bar, in a sheet that slides up from the bottom. */
export function NavDrawer({
  open,
  onClose,
  current,
  inbox,
  isHuman,
  onOpenSettings,
}: {
  open: boolean;
  onClose: () => void;
  current?: string;
  inbox?: number;
  isHuman?: boolean;
  onOpenSettings?: () => void;
}) {
  const { pathname } = useLocation();
  const panelRef = useRef<HTMLElement>(null);
  const previouslyFocused = useRef<HTMLElement | null>(null);
  const { primary, secondary } = useNavEntries(current, inbox, isHuman);
  // Close after navigating.
  const first = useRef(pathname);
  useEffect(() => {
    if (pathname !== first.current) onClose();
    first.current = pathname;
  }, [pathname, onClose]);
  // Move focus into the drawer when it opens, and restore it when it closes, so it never
  // lingers on (or returns to) a trigger that's now obscured by the backdrop.
  useEffect(() => {
    const panel = panelRef.current;
    if (open) {
      previouslyFocused.current = document.activeElement instanceof HTMLElement ? document.activeElement : null;
      const focusable = panel ? Array.from(panel.querySelectorAll<HTMLElement>(FOCUSABLE_SELECTOR)).filter((el) => el.offsetParent !== null) : [];
      (focusable[0] ?? panel)?.focus();
    } else {
      previouslyFocused.current?.focus();
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
      // Trap Tab/Shift+Tab inside the open drawer so it can't escape into the obscured page behind it.
      const focusable = Array.from(panelRef.current.querySelectorAll<HTMLElement>(FOCUSABLE_SELECTOR)).filter((el) => el.offsetParent !== null);
      if (focusable.length === 0) return;
      const [firstEl, lastEl] = [focusable[0], focusable[focusable.length - 1]];
      // Focus can start outside the panel (e.g. the drawer opened without moving focus yet,
      // or something else stole it); pull it back in instead of only wrapping at the boundaries.
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
  return (
    <div className={clsx("fixed inset-0 z-50 lg:hidden", !open && "pointer-events-none")} aria-hidden={!open} inert={!open}>
      <div className={clsx("absolute inset-0 bg-black/40 backdrop-blur-[2px] transition-opacity duration-300", open ? "opacity-100" : "opacity-0")} onClick={onClose} />
      <aside
        ref={panelRef}
        tabIndex={-1}
        className={clsx(
          "absolute inset-x-0 bottom-0 flex max-h-[88dvh] flex-col gap-4 overflow-y-auto rounded-t-3xl border-t border-line bg-surface px-4 pt-2 pb-[calc(1rem+env(safe-area-inset-bottom))] shadow-overlay outline-none transition-transform duration-300 ease-[cubic-bezier(0.32,0.72,0,1)]",
          open ? "translate-y-0" : "translate-y-full",
        )}
      >
        <div className="mx-auto h-1 w-10 shrink-0 rounded-full bg-line-strong" aria-hidden />
        <div className="flex items-center justify-between">
          <Link to={current ? `/p/${current}` : "/"} onClick={onClose} className="flex items-center gap-2 text-[15px] font-semibold tracking-tight">
            <Logo size={24} />
            agent-kanban
          </Link>
          <button onClick={onClose} className="grid h-9 w-9 place-items-center rounded-full bg-surface-2 text-fg-muted hover:text-fg" aria-label="Close menu">
            <X size={18} />
          </button>
        </div>
        <ProjectSwitcher current={current} />
        <nav className="grid grid-cols-3 gap-2">
          {[...primary, ...secondary].map((e) => (
            <NavLink
              key={e.to}
              to={e.to}
              end={e.end}
              className={({ isActive }) =>
                clsx(
                  "relative flex h-[72px] flex-col items-center justify-center gap-1.5 rounded-2xl border text-xs font-medium transition-colors",
                  isActive ? "border-accent/30 bg-accent-soft text-accent-fg" : "border-line bg-surface-2/60 text-fg-muted active:bg-surface-3",
                )
              }
            >
              <e.icon size={20} strokeWidth={1.9} />
              {e.label}
              {!!e.badge && <Badge n={e.badge} live={e.live} className="absolute top-1.5 right-1.5" />}
            </NavLink>
          ))}
        </nav>
        <SchedulerControl />
        <div className="flex items-center gap-2">
          {onOpenSettings && (
            <Button variant="default" className="flex-1 justify-center" onClick={onOpenSettings}>
              <SunMoon size={15} /> Appearance
            </Button>
          )}
          <a href="/api/docs" target="_blank" className="inline-flex h-9 flex-1 items-center justify-center gap-2 rounded-lg border border-line bg-surface text-sm font-medium text-fg-muted shadow-card hover:text-fg">
            <BookOpen size={15} /> API docs
          </a>
        </div>
      </aside>
    </div>
  );
}

const MORE_ROUTES = ["/agents", "/usage", "/server", "/settings", "/new-project"];

export function Layout({ children }: { children: ReactNode }) {
  const connected = useLiveEvents();
  const { slug } = useParams();
  const { pathname } = useLocation();
  const projects = useProjects();
  const current = slug ?? projects.data?.[0]?.slug;
  const inbox = useInboxCount(current);
  const [menu, setMenu] = useState(false);
  const closeMenu = useCallback(() => setMenu(false), []);
  const me = useMe();
  const isHuman = me.data?.kind === "human";
  const [theme, setTheme] = useThemePreference();
  const [settingsOpen, setSettingsOpen] = useState(false);
  return (
    <div className="flex h-dvh overflow-hidden bg-canvas">
      <Sidebar current={current} inbox={inbox.data} isHuman={isHuman} connected={connected} onOpenSettings={() => setSettingsOpen(true)} />
      <div className="flex min-w-0 flex-1 flex-col lg:py-2 lg:pr-2">
        <TopBar current={current} connected={connected} onMenu={() => setMenu(true)} />
        <main className="relative min-h-0 flex-1 overflow-auto lg:rounded-xl lg:border lg:border-line lg:bg-surface lg:shadow-card">{children}</main>
        <TabBar current={current} inbox={inbox.data} onMore={() => setMenu(true)} moreActive={MORE_ROUTES.some((r) => pathname.endsWith(r) || pathname.startsWith(r))} />
      </div>
      <NavDrawer
        open={menu}
        onClose={closeMenu}
        current={current}
        inbox={inbox.data}
        isHuman={isHuman}
        onOpenSettings={() => {
          setMenu(false);
          setSettingsOpen(true);
        }}
      />
      <RunToasts />
      <ConnectClaudeModal />
      <BrowserSettingsModal open={settingsOpen} onClose={() => setSettingsOpen(false)} theme={theme} onThemeChange={setTheme} />
    </div>
  );
}

export function AuthGate({ children }: { children: ReactNode }) {
  const me = useQuery({ queryKey: ["me"], queryFn: () => unwrap(client.GET("/api/me")), retry: false });
  const [token, setToken] = useState("");
  if (me.isLoading) return null;
  if (me.error instanceof ApiError && me.error.status === 401) {
    return (
      <div className="flex min-h-full items-center justify-center bg-canvas p-5">
        <div className="w-full max-w-sm animate-pop-in space-y-5 rounded-2xl border border-line bg-surface p-7 shadow-raised">
          <div className="flex flex-col items-center gap-3 text-center">
            <Logo size={44} />
            <div>
              <h1 className="text-lg font-semibold tracking-tight">Sign in to agent-kanban</h1>
              <p className="mt-1 text-sm text-fg-muted">
                Open the login link printed when the server started, or run <code className="rounded bg-surface-3 px-1 font-mono text-xs">agent-kanban login-url</code>.
              </p>
            </div>
          </div>
          <form
            onSubmit={(e) => {
              e.preventDefault();
              window.location.href = `/login?t=${encodeURIComponent(token.trim())}`;
            }}
            className="space-y-2.5"
          >
            <input className={clsx(inputCls, "font-mono")} value={token} onChange={(e) => setToken(e.target.value)} placeholder="Or paste the admin token (akh_…)" aria-label="Admin token" />
            <Button variant="primary" className="w-full justify-center" disabled={!token.trim()}>
              Log in
            </Button>
          </form>
        </div>
      </div>
    );
  }
  return <>{children}</>;
}

export async function quickPatchSettings(body: Record<string, unknown>) {
  return api("PATCH", "/api/settings", body);
}

