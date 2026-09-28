import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import clsx from "clsx";
import { Bot, BookOpen, Inbox, KanbanSquare, Play, Settings2, Square, Terminal } from "lucide-react";
import { type ReactNode, useEffect, useRef, useState } from "react";
import { Link, NavLink, useNavigate, useParams } from "react-router";
import { ApiError, ROLE_LABEL, type RunView, api, client, unwrap } from "../api/client";
import { useLiveEvents } from "../api/live";
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

function NavItem({ to, icon, children, badge }: { to: string; icon: ReactNode; children: ReactNode; badge?: number }) {
  return (
    <NavLink
      to={to}
      end={to.split("/").length <= 3}
      className={({ isActive }) =>
        clsx(
          "flex items-center gap-1.5 rounded-md px-2.5 py-1.5 text-sm",
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

export function Layout({ children }: { children: ReactNode }) {
  const connected = useLiveEvents();
  const { slug } = useParams();
  const projects = useProjects();
  const nav = useNavigate();
  const current = slug ?? projects.data?.[0]?.slug;
  const inbox = useInboxCount(current);
  return (
    <div className="flex h-full flex-col">
      <header className="flex items-center gap-3 border-b border-zinc-200 dark:border-zinc-800 bg-white dark:bg-zinc-950 px-4 py-2">
        <span className="font-semibold tracking-tight">🗂️ agent-kanban</span>
        {projects.data && projects.data.length > 0 && (
          <select
            className="rounded-md border border-zinc-300 dark:border-zinc-700 bg-transparent px-2 py-1 text-sm"
            value={current}
            onChange={(e) => nav(`/p/${e.target.value}`)}
          >
            {projects.data.map((p) => (
              <option key={p.slug} value={p.slug}>
                {p.name}
              </option>
            ))}
          </select>
        )}
        <nav className="flex items-center gap-1">
          {current && (
            <>
              <NavItem to={`/p/${current}`} icon={<KanbanSquare size={15} />}>
                Board
              </NavItem>
              <NavItem to={`/p/${current}/inbox`} icon={<Inbox size={15} />} badge={inbox.data}>
                Inbox
              </NavItem>
            </>
          )}
          <NavItem to="/runs" icon={<Terminal size={15} />}>
            Runs
          </NavItem>
          <NavItem to="/agents" icon={<Bot size={15} />}>
            Agents
          </NavItem>
          {current && (
            <NavItem to={`/p/${current}/settings`} icon={<Settings2 size={15} />}>
              Project
            </NavItem>
          )}
        </nav>
        <div className="ml-auto flex items-center gap-3">
          <SchedulerControl />
          <a href="/api/docs" target="_blank" className="text-zinc-500 hover:text-zinc-900 dark:hover:text-zinc-100 text-xs flex items-center gap-1">
            <BookOpen size={13} /> API
          </a>
          <span title={connected ? "Live updates connected" : "Reconnecting…"} className={clsx("h-2 w-2 rounded-full", connected ? "bg-emerald-500" : "bg-zinc-400 animate-pulse")} />
        </div>
      </header>
      <main className="min-h-0 flex-1 overflow-auto">{children}</main>
      <RunToasts />
    </div>
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
