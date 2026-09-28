import {
  DndContext,
  type DragEndEvent,
  DragOverlay,
  type DragStartEvent,
  PointerSensor,
  closestCorners,
  useDroppable,
  useSensor,
  useSensors,
} from "@dnd-kit/core";
import { SortableContext, useSortable, verticalListSortingStrategy } from "@dnd-kit/sortable";
import { CSS } from "@dnd-kit/utilities";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import clsx from "clsx";
import { AlertTriangle, Check, Clock, GitMerge, GitPullRequest, MessageSquare, Plus, Search } from "lucide-react";
import { useMemo, useState } from "react";
import { Outlet, useNavigate, useParams, useSearchParams } from "react-router";
import { type Card as CardT, COLUMN_STATES, type IssueState, ROLE_LABEL, type Role, STATE_LABEL, api, client, unwrap } from "../api/client";
import { Button, ErrorBox, Field, HoldBadge, LabelChip, Modal, Pill, StateBadge, fieldCls, inputCls, timeAgo } from "../components/ui";

const PRIORITY_STYLE: Record<string, string> = {
  P0: "bg-red-600 text-white",
  P1: "bg-orange-500 text-white",
  P2: "bg-zinc-300 text-zinc-800 dark:bg-zinc-700 dark:text-zinc-200",
};

function CardView({ card, onOpen, dragging }: { card: CardT; onOpen?: () => void; dragging?: boolean }) {
  const running = card.run && ["queued", "preparing", "running"].includes(card.run.status);
  return (
    <div
      onClick={onOpen}
      className={clsx(
        "group cursor-pointer rounded-md border bg-white dark:bg-zinc-900 p-2.5 shadow-xs hover:border-zinc-400 dark:hover:border-zinc-600 space-y-1.5",
        card.hold === "needs_decision" ? "border-amber-400 dark:border-amber-700" : "border-zinc-200 dark:border-zinc-800",
        dragging && "shadow-lg rotate-1",
      )}
    >
      <div className="flex items-start gap-1.5">
        <span className="text-xs text-zinc-500 font-mono mt-0.5">#{card.number}</span>
        <span className="text-sm leading-snug flex-1">{card.title}</span>
        {card.priority && <Pill className={PRIORITY_STYLE[card.priority]}>{card.priority}</Pill>}
      </div>
      <div className="flex flex-wrap items-center gap-1">
        <StateBadge state={card.state} />
        {card.hold && <HoldBadge hold={card.hold} reason={card.hold_reason} />}
        {card.labels.map((l) => (
          <LabelChip key={l.name} {...l} />
        ))}
        {card.size && <Pill className="bg-zinc-100 dark:bg-zinc-800 text-zinc-600 dark:text-zinc-400">{card.size}</Pill>}
      </div>
      {(card.pr || running || card.waiting_on_limit || card.comment_count > 0 || card.failure_count > 0) && (
        <div className="flex items-center gap-2 text-[11px] text-zinc-500">
          {card.pr && (
            <span
              className={clsx(
                "inline-flex items-center gap-0.5",
                card.pr.state === "merged" && "text-violet-600",
                card.pr.has_conflicts && "text-red-600",
                card.pr.approved && "text-emerald-600",
              )}
              title={card.pr.has_conflicts ? "Merge conflicts" : card.pr.approved ? "Approved at head" : `PR ${card.pr.state}`}
            >
              {card.pr.state === "merged" ? <GitMerge size={12} /> : <GitPullRequest size={12} />}#{card.pr.number}
              {card.pr.has_conflicts && <AlertTriangle size={11} />}
              {card.pr.approved && <Check size={11} />}
              {card.pr.unresolved_threads > 0 && card.pr.state === "open" && <span title="Unresolved threads">· {card.pr.unresolved_threads}💬</span>}
            </span>
          )}
          {card.comment_count > 0 && (
            <span className="inline-flex items-center gap-0.5">
              <MessageSquare size={11} />
              {card.comment_count}
            </span>
          )}
          {card.failure_count > 0 && !card.hold && (
            <span className="text-rose-600" title={`Failed runs; next attempt ${timeAgo(card.next_attempt_at)}`}>
              ✗{card.failure_count}
            </span>
          )}
          {running && card.run && (
            <span className="ml-auto inline-flex items-center gap-1 text-blue-600 dark:text-blue-400" title={`Run #${card.run.id} ${card.run.status}`}>
              <span className="relative flex h-2 w-2">
                <span className="absolute inline-flex h-full w-full animate-ping rounded-full bg-blue-400 opacity-75" />
                <span className="relative inline-flex h-2 w-2 rounded-full bg-blue-500" />
              </span>
              {ROLE_LABEL[card.run.role as Role] ?? card.run.role} · {card.run.agent}
            </span>
          )}
          {!running && card.last_run && !card.waiting_on_limit && <LastRun r={card.last_run} />}
          {!running && card.waiting_on_limit && (
            <span className="ml-auto inline-flex items-center gap-0.5 text-amber-600" title="The agent for the next step is paused by a usage limit">
              <Clock size={11} />
              {card.waiting_on_limit.includes("T") ? `resumes ${timeAgo(card.waiting_on_limit)}` : card.waiting_on_limit}
            </span>
          )}
        </div>
      )}
    </div>
  );
}

const LAST_RUN_STYLE: Record<string, [string, string]> = {
  succeeded: ["✓", "text-emerald-600 dark:text-emerald-400"],
  failed: ["✗", "text-rose-600 dark:text-rose-400"],
  rate_limited: ["⏸", "text-amber-600 dark:text-amber-400"],
  cancelled: ["■", "text-zinc-500"],
  interrupted: ["↺", "text-zinc-500"],
};

function LastRun({ r }: { r: NonNullable<CardT["last_run"]> }) {
  const age = r.ended_at ? (Date.now() - new Date(r.ended_at).getTime()) / 1000 : Infinity;
  if (age > 86400) return null;
  const [icon, cls] = LAST_RUN_STYLE[r.status] ?? ["•", "text-zinc-500"];
  const verb = r.status === "succeeded" ? "done" : r.status.replace("_", " ");
  return (
    <span className={clsx("ml-auto inline-flex items-center gap-1", cls, age > 3600 && "opacity-60")} title={`Run #${r.id} ${r.status}${r.error ? `: ${r.error}` : ""}`}>
      {icon} {ROLE_LABEL[r.role as Role] ?? r.role} {verb} · {timeAgo(r.ended_at)}
    </span>
  );
}

function SortableCard({ card, onOpen }: { card: CardT; onOpen: () => void }) {
  const { attributes, listeners, setNodeRef, transform, transition, isDragging } = useSortable({ id: `card-${card.number}`, data: { card } });
  return (
    <div ref={setNodeRef} style={{ transform: CSS.Transform.toString(transform), transition, opacity: isDragging ? 0.4 : 1 }} {...attributes} {...listeners}>
      <CardView card={card} onOpen={onOpen} />
    </div>
  );
}

function Column({ id, title, cards, onOpen }: { id: string; title: string; cards: CardT[]; onOpen: (n: number) => void }) {
  const { setNodeRef, isOver } = useDroppable({ id: `col-${id}`, data: { column: id } });
  return (
    <div className="flex w-80 shrink-0 flex-col rounded-lg bg-zinc-100/80 dark:bg-zinc-900/50">
      <div className="flex items-center gap-2 px-3 py-2 text-sm font-semibold">
        {title}
        <span className="rounded-full bg-zinc-200 dark:bg-zinc-800 px-1.5 text-xs font-normal text-zinc-600 dark:text-zinc-400">{cards.length}</span>
      </div>
      <div ref={setNodeRef} className={clsx("flex-1 space-y-2 overflow-y-auto px-2 pb-2 min-h-24", isOver && "bg-blue-50/60 dark:bg-blue-950/20 rounded-b-lg")}>
        <SortableContext items={cards.map((c) => `card-${c.number}`)} strategy={verticalListSortingStrategy}>
          {cards.map((c) => (
            <SortableCard key={c.number} card={c} onOpen={() => onOpen(c.number)} />
          ))}
        </SortableContext>
      </div>
    </div>
  );
}

type Drop = { card: CardT; column: string };

function StatePicker({ drop, onClose, onPick }: { drop: Drop | null; onClose: () => void; onPick: (s: IssueState, reason?: string) => void }) {
  const [reason, setReason] = useState("wontfix");
  if (!drop) return null;
  const states = COLUMN_STATES[drop.column].filter((s) => s !== drop.card.state);
  return (
    <Modal open onClose={onClose} title={`Move #${drop.card.number} to…`}>
      <div className="space-y-2">
        {states.map((s) =>
          s === "closed" ? (
            <div key={s} className="flex gap-2">
              <select className={inputCls} value={reason} onChange={(e) => setReason(e.target.value)}>
                {["wontfix", "duplicate", "invalid", "not_planned"].map((r) => (
                  <option key={r}>{r}</option>
                ))}
              </select>
              <Button onClick={() => onPick("closed", reason)}>Close as {reason}</Button>
            </div>
          ) : (
            <Button key={s} className="w-full justify-start" onClick={() => onPick(s)}>
              <StateBadge state={s} always />
              <span className="text-xs text-zinc-500">
                {s === "triage" && "A triage agent will look at it"}
                {s === "ready" && "A fix agent will pick it up"}
                {s === "backlog" && "Park it; no agent will touch it"}
                {s === "done" && "Mark completed (cleans up worktree)"}
              </span>
            </Button>
          ),
        )}
      </div>
    </Modal>
  );
}

export function NewIssueModal({ slug, open, onClose }: { slug: string; open: boolean; onClose: () => void }) {
  const qc = useQueryClient();
  const [title, setTitle] = useState("");
  const [body, setBody] = useState("");
  const [labels, setLabels] = useState("bug");
  const [priority, setPriority] = useState("");
  const [state, setState] = useState<IssueState>("triage");
  const create = useMutation({
    mutationFn: () =>
      unwrap(
        client.POST("/api/projects/{p}/issues", {
          params: { path: { p: slug } },
          body: {
            title,
            body,
            labels: labels.split(",").map((s) => s.trim()).filter(Boolean),
            priority: priority || null,
            state,
          } as never,
        }),
      ),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["board"] });
      setTitle("");
      setBody("");
      onClose();
    },
  });
  return (
    <Modal open={open} onClose={onClose} title="New issue" wide>
      <form
        className="space-y-3"
        onSubmit={(e) => {
          e.preventDefault();
          create.mutate();
        }}
      >
        <Field label="Title">
          <input autoFocus className={inputCls} value={title} onChange={(e) => setTitle(e.target.value)} />
        </Field>
        <Field label="Description (markdown)">
          <textarea className={clsx(inputCls, "font-mono h-40")} value={body} onChange={(e) => setBody(e.target.value)} />
        </Field>
        <div className="grid grid-cols-3 gap-3">
          <Field label="Labels (comma-separated)">
            <input className={inputCls} value={labels} onChange={(e) => setLabels(e.target.value)} />
          </Field>
          <Field label="Priority">
            <select className={inputCls} value={priority} onChange={(e) => setPriority(e.target.value)}>
              <option value="">—</option>
              <option>P0</option>
              <option>P1</option>
              <option>P2</option>
            </select>
          </Field>
          <Field label="Start in">
            <select className={inputCls} value={state} onChange={(e) => setState(e.target.value as IssueState)}>
              {(["triage", "backlog", "ready"] as IssueState[]).map((s) => (
                <option key={s} value={s}>
                  {STATE_LABEL[s]}
                </option>
              ))}
            </select>
          </Field>
        </div>
        <ErrorBox error={create.error} />
        <div className="flex justify-end">
          <Button variant="primary" disabled={!title.trim() || create.isPending}>
            Create issue
          </Button>
        </div>
      </form>
    </Modal>
  );
}

const BADGES = ["triage", "ready", "changes_requested", "merge_conflict", "ready_to_merge", "needs_decision", "stalled", "paused", "closed"];

export default function BoardPage() {
  const { slug = "" } = useParams();
  const nav = useNavigate();
  const qc = useQueryClient();
  const [sp, setSp] = useSearchParams();
  const [q, setQ] = useState(sp.get("q") ?? "");
  const [newOpen, setNewOpen] = useState(false);
  const [drop, setDrop] = useState<Drop | null>(null);
  const [active, setActive] = useState<CardT | null>(null);
  const [moveError, setMoveError] = useState<unknown>(null);
  const filters = {
    label: sp.get("label") || undefined,
    badge: sp.get("badge") || undefined,
    agent: sp.get("agent") || undefined,
    running: sp.get("running") === "1" ? true : undefined,
    q: sp.get("q") || undefined,
  };
  const board = useQuery({
    queryKey: ["board", slug, filters],
    queryFn: () => unwrap(client.GET("/api/projects/{p}/board", { params: { path: { p: slug }, query: filters } })),
    refetchInterval: 30000,
  });
  const agents = useQuery({ queryKey: ["agents"], queryFn: () => unwrap(client.GET("/api/agents")) });
  const sensors = useSensors(useSensor(PointerSensor, { activationConstraint: { distance: 6 } }));

  const transition = useMutation({
    mutationFn: ({ n, to, reason }: { n: number; to: IssueState; reason?: string }) =>
      api("POST", `/api/projects/${slug}/issues/${n}/transition`, { to, close_reason: reason }),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["board"] }),
    onError: (e) => setMoveError(e),
  });
  const rerank = useMutation({
    mutationFn: ({ n, rank }: { n: number; rank: number }) => api("PATCH", `/api/projects/${slug}/issues/${n}`, { rank }),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["board"] }),
  });

  const colOf = useMemo(() => {
    const m = new Map<number, string>();
    board.data?.columns.forEach((c) => c.cards.forEach((card) => m.set(card.number, c.id)));
    return m;
  }, [board.data]);

  const setFilter = (k: string, v?: string) => {
    const next = new URLSearchParams(sp);
    if (v) next.set(k, v);
    else next.delete(k);
    setSp(next, { replace: true });
  };

  const onDragStart = (e: DragStartEvent) => setActive((e.active.data.current?.card as CardT) ?? null);
  const onDragEnd = (e: DragEndEvent) => {
    setActive(null);
    const card = e.active.data.current?.card as CardT | undefined;
    if (!card || !e.over || !board.data) return;
    const overCard = e.over.data.current?.card as CardT | undefined;
    const target = (e.over.data.current?.column as string | undefined) ?? (overCard ? colOf.get(overCard.number) : undefined);
    const source = colOf.get(card.number);
    if (!target) return;
    if (target === source) {
      if (!overCard || overCard.number === card.number || target === "done") return;
      const cards = board.data.columns.find((c) => c.id === target)!.cards;
      const oldIdx = cards.findIndex((c) => c.number === card.number);
      const newIdx = cards.findIndex((c) => c.number === overCard.number);
      const without = cards.filter((c) => c.number !== card.number);
      const before = newIdx > oldIdx ? without[newIdx - 1] : without[newIdx - 1];
      const after = newIdx > oldIdx ? without[newIdx] : without[newIdx];
      const lo = before?.rank ?? (after ? after.rank - 2 : 0);
      const hi = after?.rank ?? lo + 2;
      rerank.mutate({ n: card.number, rank: (lo + hi) / 2 });
      return;
    }
    if (target === "in_progress") transition.mutate({ n: card.number, to: "in_progress" });
    else if (target === "in_review") transition.mutate({ n: card.number, to: "in_review" });
    else setDrop({ card, column: target });
  };

  const labels = board.data?.labels ?? [];
  const pausedGroups = board.data?.limit_groups.filter((g) => g.paused) ?? [];

  return (
    <div className="flex h-full flex-col">
      <div className="flex flex-wrap items-center gap-2 border-b border-zinc-200 dark:border-zinc-800 px-4 py-2">
        <form
          onSubmit={(e) => {
            e.preventDefault();
            setFilter("q", q || undefined);
          }}
          className="relative"
        >
          <Search size={13} className="absolute left-2 top-2 text-zinc-400" />
          <input className={clsx(fieldCls, "pl-7 w-56 py-1")} placeholder="Search or #number" value={q} onChange={(e) => setQ(e.target.value)} />
        </form>
        <select className={clsx(fieldCls, " py-1")} value={filters.label ?? ""} onChange={(e) => setFilter("label", e.target.value)}>
          <option value="">All labels</option>
          {labels.map((l) => (
            <option key={l.name}>{l.name}</option>
          ))}
        </select>
        <select className={clsx(fieldCls, " py-1")} value={filters.badge ?? ""} onChange={(e) => setFilter("badge", e.target.value)}>
          <option value="">All states</option>
          {BADGES.map((b) => (
            <option key={b} value={b}>
              {b.replace(/_/g, " ")}
            </option>
          ))}
        </select>
        <select className={clsx(fieldCls, " py-1")} value={filters.agent ?? ""} onChange={(e) => setFilter("agent", e.target.value)}>
          <option value="">Any agent</option>
          {agents.data?.map((a) => (
            <option key={a.slug} value={a.slug}>
              {a.name}
            </option>
          ))}
        </select>
        <label className="flex items-center gap-1 text-sm text-zinc-600 dark:text-zinc-400">
          <input type="checkbox" checked={!!filters.running} onChange={(e) => setFilter("running", e.target.checked ? "1" : undefined)} />
          Running
        </label>
        {pausedGroups.map((g) => (
          <Pill key={g.name} className="bg-amber-100 text-amber-900 dark:bg-amber-950 dark:text-amber-300" title={g.pause_reason ?? ""}>
            <Clock size={11} /> {g.name} paused{g.paused_until ? ` · resumes ${timeAgo(g.paused_until)}` : ""}
          </Pill>
        ))}
        <div className="ml-auto">
          <Button variant="primary" onClick={() => setNewOpen(true)}>
            <Plus size={14} /> New issue
          </Button>
        </div>
      </div>
      {moveError ? (
        <div className="px-4 pt-2" onClick={() => setMoveError(null)}>
          <ErrorBox error={moveError} />
        </div>
      ) : null}
      <ErrorBox error={board.error} />
      <div className="flex min-h-0 flex-1 gap-3 overflow-x-auto p-4">
        <DndContext sensors={sensors} collisionDetection={closestCorners} onDragStart={onDragStart} onDragEnd={onDragEnd} onDragCancel={() => setActive(null)}>
          {board.data?.columns.map((c) => (
            <Column key={c.id} id={c.id} title={c.title} cards={c.cards} onOpen={(n) => nav(`/p/${slug}/issues/${n}${window.location.search}`)} />
          ))}
          <DragOverlay>{active ? <CardView card={active} dragging /> : null}</DragOverlay>
        </DndContext>
      </div>
      <StatePicker
        drop={drop}
        onClose={() => setDrop(null)}
        onPick={(to, reason) => {
          if (drop) transition.mutate({ n: drop.card.number, to, reason });
          setDrop(null);
        }}
      />
      <NewIssueModal slug={slug} open={newOpen} onClose={() => setNewOpen(false)} />
      <Outlet />
    </div>
  );
}
