import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import clsx from "clsx";
import { BarChart3, Bot, BookOpen, Inbox, KanbanSquare, Menu, Play, Settings2, Square, Terminal, X } from "lucide-react";
import { type ReactNode, useCallback, useEffect, useRef, useState } from "react";
import { Link, NavLink, useLocation, useNavigate, useParams } from "react-router";
import { ApiError, ROLE_LABEL, type RunView, api, client, unwrap } from "../api/client";
import { useLiveEvents } from "../api/live";
import { ConnectClaudeModal } from "./ConnectClaude";
import { Button, inputCls } from "./ui";

export function useProjects() {
  return useQuery({ queryKey: ["projects"], queryFn: () => unwrap(client.GET("/api/projects")) });
}

export function useSettings() {
  return useQuery({ queryKey: ["settings"], queryFn: () => unwrap(client.GET("/api/settings")) });
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

function NavItem({ to, icon, children, badge, large }: { to: string; icon: ReactNode; children: ReactNode; badge?: number; large?: boolean }) {
  return (
    <NavLink
      to={to}
      end={to.split("/").length <= 3}
      className={({ isActive }) =>
        clsx(
          "flex items-center rounded-md text-sm",
          large ? "gap-2.5 px-3 py-2.5" : "gap-1.5 px-2.5 py-1.5",
          isActive ? "bg-zinc-200/70 dark:bg-zinc-800 font-medium" : "text-zinc-600 dark:text-zinc-400 hover:bg-zinc-100 dark:hover:bg-zinc-900",
        )
      }
    >
      {icon}
      {children}
      {!!badge && <span className="ml-0.5 rounded-full bg-amber-500 text-white text-[10px] px-1.5 leading-4">{badge}</span>}
    </NavLink>
  );
}

function SchedulerControl() {
  const qc = useQueryClient();
  const settings = useSettings();
  const runs = useQuery({
    queryKey: ["runs", "active"],
    queryFn: () => unwrap(client.GET("/api/runs", { params: { query: { status: "active" } } })),
    refetchInterval: 15000,
  });
  const toggle = useMutation({
    mutationFn: (enabled: boolean) => unwrap(client.PATCH("/api/settings", { body: { scheduler_enabled: enabled } })),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["settings"] }),
  });
  if (!settings.data) return null;
  const on = settings.data.scheduler_enabled;
  return (
    <div className="flex items-center gap-2 text-xs">
      <span className="text-zinc-500" title="Active agent runs / concurrency limit">
        <Bot size={13} className="inline -mt-0.5" /> {runs.data?.length ?? 0}/{settings.data.max_concurrent_runs}
      </span>
      <Button
        size="sm"
        variant={on ? "success" : "default"}
        onClick={() => toggle.mutate(!on)}
        title={on ? "Agents are being dispatched automatically. Click to stop dispatching new runs." : "Automatic dispatch is off. Click to start."}
      >
        {on ? <Square size={11} /> : <Play size={11} />}
        {on ? "Scheduler on" : "Scheduler off"}
      </Button>
    </div>
  );
}

const TOAST_STYLE: Record<string, string> = {
  succeeded: "border-emerald-300 dark:border-emerald-800",
  failed: "border-rose-300 dark:border-rose-800",
  rate_limited: "border-amber-300 dark:border-amber-800",
};

/** Pops a toast when an agent run finishes, and shows the running count in the tab title. */
function RunToasts() {
  const runs = useQuery({
    queryKey: ["runs", "active"],
    queryFn: () => unwrap(client.GET("/api/runs", { params: { query: { status: "active" } } })),
    refetchInterval: 15000,
  });
  const prev = useRef<Set<number> | null>(null);
  const [toasts, setToasts] = useState<RunView[]>([]);
  useEffect(() => {
    if (!runs.data) return;
    const now = new Set(runs.data.map((r) => r.id));
    const gone = prev.current ? [...prev.current].filter((id) => !now.has(id)) : [];
    prev.current = now;
    document.title = now.size ? `(${now.size} running) agent-kanban` : "agent-kanban";
    for (const id of gone) {
      unwrap(client.GET("/api/runs/{id}", { params: { path: { id } } })).then((r) => {
        setToasts((t) => [...t, r]);
        setTimeout(() => setToasts((t) => t.filter((x) => x.id !== r.id)), 10000);
        if (document.hidden && "Notification" in window && Notification.permission === "granted") {
          new Notification(`${ROLE_LABEL[r.role]} run ${r.status.replace("_", " ")}`, { body: `#${r.issue} ${r.issue_title ?? ""}` });
        }
      });
    }
  }, [runs.data]);
  useEffect(() => {
    if ("Notification" in window && Notification.permission === "default") {
      const ask = () => Notification.requestPermission();
      window.addEventListener("click", ask, { once: true });
      return () => window.removeEventListener("click", ask);
    }
  }, []);
  return (
    <div className="fixed bottom-4 right-4 z-50 space-y-2">
      {toasts.map((r) => (
        <Link
          key={r.id}
          to={`/runs/${r.id}`}
          className={clsx("block w-80 rounded-lg border-l-4 border bg-white dark:bg-zinc-900 px-3 py-2 shadow-lg text-sm", TOAST_STYLE[r.status] ?? "border-zinc-300 dark:border-zinc-700")}
        >
          <div className="font-medium">
            {r.status === "succeeded" ? "✓" : r.status === "failed" ? "✗" : r.status === "rate_limited" ? "⏸" : "■"} {ROLE_LABEL[r.role]} run #{r.id} {r.status.replace("_", " ")}
          </div>
          <div className="truncate text-xs text-zinc-500">
            {r.agent} · #{r.issue} {r.issue_title}
          </div>
          {r.error && <div className="truncate text-xs text-rose-600">{r.error}</div>}
        </Link>
      ))}
    </div>
  );
}

function NavLinks({ current, inbox, large }: { current?: string; inbox?: number; large?: boolean }) {
  const size = large ? 17 : 15;
  return (
    <>
      {current && (
        <>
          <NavItem large={large} to={`/p/${current}`} icon={<KanbanSquare size={size} />}>
            Board
          </NavItem>
          <NavItem large={large} to={`/p/${current}/inbox`} icon={<Inbox size={size} />} badge={inbox}>
            Inbox
          </NavItem>
        </>
      )}
      <NavItem large={large} to="/runs" icon={<Terminal size={size} />}>
        Runs
      </NavItem>
      <NavItem large={large} to="/agents" icon={<Bot size={size} />}>
        Agents
      </NavItem>
      <NavItem large={large} to="/usage" icon={<BarChart3 size={size} />}>
        Usage
      </NavItem>
      {current && (
        <NavItem large={large} to={`/p/${current}/settings`} icon={<Settings2 size={size} />}>
          Project
        </NavItem>
      )}
    </>
  );
}

function ProjectSelect({ current, className }: { current?: string; className?: string }) {
  const projects = useProjects();
  const nav = useNavigate();
  if (!projects.data || projects.data.length === 0) return null;
  return (
    <select
      className={clsx("rounded-md border border-zinc-300 dark:border-zinc-700 bg-transparent px-2 py-1 text-sm", className)}
      value={current}
      onChange={(e) => nav(`/p/${e.target.value}`)}
    >
      {projects.data.map((p) => (
        <option key={p.slug} value={p.slug}>
          {p.name}
        </option>
      ))}
    </select>
  );
}

function LiveDot({ connected }: { connected: boolean }) {
  return (
    <span title={connected ? "Live updates connected" : "Reconnecting…"} className={clsx("h-2 w-2 shrink-0 rounded-full", connected ? "bg-emerald-500" : "bg-zinc-400 animate-pulse")} />
  );
}

/** Small screens: the nav lives in a drawer that slides in from the left. */
function NavDrawer({ open, onClose, current, inbox }: { open: boolean; onClose: () => void; current?: string; inbox?: number }) {
  const { pathname } = useLocation();
  // Close after navigating.
  const first = useRef(pathname);
  useEffect(() => {
    if (pathname !== first.current) onClose();
    first.current = pathname;
  }, [pathname, onClose]);
  useEffect(() => {
    if (!open) return;
    const k = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("keydown", k);
    return () => window.removeEventListener("keydown", k);
  }, [open, onClose]);
  return (
    <div className={clsx("fixed inset-0 z-50 lg:hidden", !open && "pointer-events-none")} aria-hidden={!open}>
      <div className={clsx("absolute inset-0 bg-black/40 transition-opacity", open ? "opacity-100" : "opacity-0")} onClick={onClose} />
      <aside
        className={clsx(
          "absolute inset-y-0 left-0 flex w-72 max-w-[85vw] flex-col gap-4 border-r border-zinc-200 dark:border-zinc-800 bg-white dark:bg-zinc-950 p-3 shadow-xl transition-transform",
          open ? "translate-x-0" : "-translate-x-full",
        )}
      >
        <div className="flex items-center justify-between px-1">
          <span className="font-semibold tracking-tight">🗂️ agent-kanban</span>
          <button onClick={onClose} className="rounded-md p-2 text-zinc-500 hover:bg-zinc-100 dark:hover:bg-zinc-900" aria-label="Close menu">
            <X size={18} />
          </button>
        </div>
        <ProjectSelect current={current} className="w-full py-2" />
        <nav className="flex flex-col gap-0.5">
          <NavLinks current={current} inbox={inbox} large />
        </nav>
        <div className="mt-auto space-y-3 border-t border-zinc-200 dark:border-zinc-800 px-1 pt-3">
          <SchedulerControl />
          <a href="/api/docs" target="_blank" className="flex items-center gap-1.5 text-sm text-zinc-500 hover:text-zinc-900 dark:hover:text-zinc-100">
            <BookOpen size={15} /> API docs
          </a>
        </div>
      </aside>
    </div>
  );
}

export function Layout({ children }: { children: ReactNode }) {
  const connected = useLiveEvents();
  const { slug } = useParams();
  const projects = useProjects();
  const current = slug ?? projects.data?.[0]?.slug;
  const inbox = useInboxCount(current);
  const [menu, setMenu] = useState(false);
  const closeMenu = useCallback(() => setMenu(false), []);
  return (
    <div className="flex h-full flex-col">
      <header className="flex items-center gap-2 border-b border-zinc-200 dark:border-zinc-800 bg-white dark:bg-zinc-950 px-2 py-1.5 lg:gap-3 lg:px-4 lg:py-2">
        <button onClick={() => setMenu(true)} className="relative rounded-md p-2 text-zinc-600 dark:text-zinc-400 hover:bg-zinc-100 dark:hover:bg-zinc-900 lg:hidden" aria-label="Menu">
          <Menu size={18} />
          {!!inbox.data && <span className="absolute right-1 top-1 h-2 w-2 rounded-full bg-amber-500" />}
        </button>
        <span className="whitespace-nowrap font-semibold tracking-tight">🗂️ agent-kanban</span>
        <ProjectSelect current={current} className="hidden lg:block" />
        <nav className="hidden items-center gap-1 lg:flex">
          <NavLinks current={current} inbox={inbox.data} />
        </nav>
        <div className="ml-auto flex items-center gap-3">
          <div className="hidden lg:block">
            <SchedulerControl />
          </div>
          <SchedulerBadge />
          <a href="/api/docs" target="_blank" className="hidden lg:flex text-zinc-500 hover:text-zinc-900 dark:hover:text-zinc-100 text-xs items-center gap-1">
            <BookOpen size={13} /> API
          </a>
          <LiveDot connected={connected} />
        </div>
      </header>
      <NavDrawer open={menu} onClose={closeMenu} current={current} inbox={inbox.data} />
      <main className="min-h-0 flex-1 overflow-auto">{children}</main>
      <RunToasts />
      <ConnectClaudeModal />
    </div>
  );
}

/** Small screens: running count and whether the scheduler is on, at a glance (the switch is in the menu). */
function SchedulerBadge() {
  const settings = useSettings();
  const runs = useQuery({
    queryKey: ["runs", "active"],
    queryFn: () => unwrap(client.GET("/api/runs", { params: { query: { status: "active" } } })),
    refetchInterval: 15000,
  });
  if (!settings.data) return null;
  const on = settings.data.scheduler_enabled;
  return (
    <span
      className={clsx("flex items-center gap-1 rounded-full px-2 py-0.5 text-xs lg:hidden", on ? "bg-emerald-100 text-emerald-800 dark:bg-emerald-950 dark:text-emerald-300" : "bg-zinc-100 text-zinc-500 dark:bg-zinc-900")}
      title={on ? "Scheduler on" : "Scheduler off"}
    >
      <Bot size={12} /> {runs.data?.length ?? 0}/{settings.data.max_concurrent_runs}
    </span>
  );
}

export function AuthGate({ children }: { children: ReactNode }) {
  const me = useQuery({ queryKey: ["me"], queryFn: () => unwrap(client.GET("/api/me")), retry: false });
  const [token, setToken] = useState("");
  if (me.isLoading) return null;
  if (me.error instanceof ApiError && me.error.status === 401) {
    return (
      <div className="flex h-full items-center justify-center p-6">
        <div className="max-w-md w-full space-y-3 rounded-lg border border-zinc-200 dark:border-zinc-800 bg-white dark:bg-zinc-900 p-6">
          <h1 className="text-lg font-semibold">🗂️ agent-kanban</h1>
          <p className="text-sm text-zinc-600 dark:text-zinc-400">
            Open the login link printed when the server started, or run <code className="font-mono">agent-kanban login-url</code>. You can also paste the admin token:
          </p>
          <form
            onSubmit={(e) => {
              e.preventDefault();
              window.location.href = `/login?t=${encodeURIComponent(token.trim())}`;
            }}
            className="flex gap-2"
          >
            <input className={inputCls} value={token} onChange={(e) => setToken(e.target.value)} placeholder="akh_…" />
            <Button variant="primary">Log in</Button>
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
