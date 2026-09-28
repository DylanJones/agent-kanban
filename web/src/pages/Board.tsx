import {
  DndContext,
  type DragEndEvent,
  DragOverlay,
  type DragStartEvent,
  MouseSensor,
  TouchSensor,
  closestCorners,
  useDroppable,
  useSensor,
  useSensors,
} from "@dnd-kit/core";
import { SortableContext, useSortable, verticalListSortingStrategy } from "@dnd-kit/sortable";
import { CSS } from "@dnd-kit/utilities";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import clsx from "clsx";
import { AlertTriangle, Ban, Bot, Check, Clock, GitMerge, GitPullRequest, KeyRound, MessageSquare, Moon, Plus, Search, SlidersHorizontal, UserRound } from "lucide-react";
import { useMemo, useState } from "react";
import { Outlet, useNavigate, useParams, useSearchParams } from "react-router";
import { type Card as CardT, COLUMN_STATES, type IssueState, ROLE_LABEL, type Role, STATE_LABEL, api, client, unwrap } from "../api/client";
import { useConnectClaude } from "../components/ConnectClaude";
import { ImageTextarea } from "../components/ImageTextarea";
import { Button, ErrorBox, Field, HoldBadge, LabelChip, Modal, Pill, StateBadge, fieldCls, inputCls, timeAgo } from "../components/ui";

const PRIORITY_STYLE: Record<string, string> = {
  P0: "bg-red-600 text-white",
  P1: "bg-orange-500 text-white",
  P2: "bg-zinc-300 text-zinc-800 dark:bg-zinc-700 dark:text-zinc-200",
};

type Next = CardT["next"];

/** Left accent colour by what happens next. */
const NEXT_ACCENT: Record<string, string> = {
  agent: "border-l-blue-500",
  running: "border-l-blue-500",
  waiting: "border-l-amber-400",
  human: "border-l-amber-500",
  blocked: "border-l-rose-500",
  parked: "border-l-zinc-300 dark:border-l-zinc-700",
  done: "border-l-transparent",
};

function ConnectChip({ label, title }: { label: string; title: string }) {
  const connect = useConnectClaude();
  return (
    <button
      className="inline-flex items-center gap-1 rounded text-[11px] font-medium text-rose-600 hover:underline dark:text-rose-400"
      title={title}
      onClick={(e) => {
        e.stopPropagation();
        connect();
      }}
      onPointerDown={(e) => e.stopPropagation()}
    >
      <KeyRound size={11} /> {label} · Connect
    </button>
  );
}

function ConnectBanner({ count }: { count: number }) {
  const connect = useConnectClaude();
  return (
    <div className="mx-4 mt-2 flex flex-wrap items-center gap-2 rounded-md border border-rose-300 bg-rose-50 px-3 py-1.5 text-sm dark:border-rose-900 dark:bg-rose-950/30">
      <KeyRound size={14} className="text-rose-600" />
      <span>
        <b>{count}</b> issue{count > 1 ? "s are" : " is"} waiting for Claude, which can't sign in inside containers until it's connected.
      </span>
      <Button size="sm" variant="primary" className="ml-auto" onClick={connect}>
        Connect Claude
      </Button>
    </div>
  );
}

function NextChip({ next, schedulerOn }: { next: Next; schedulerOn: boolean }) {
  if (next.kind === "done" || !next.label) return null;
  const until = next.until ? ` · ${timeAgo(next.until)}` : "";
  const title = [next.detail, next.kind === "agent" && !schedulerOn ? "The scheduler is off, so nothing starts until you turn it on or press Run." : ""]
    .filter(Boolean)
    .join(" ");
  const base = "inline-flex items-center gap-1 text-[11px] font-medium";
  switch (next.kind) {
    case "running":
      return (
        <span className={clsx(base, "text-blue-600 dark:text-blue-400")} title={title}>
          <span className="relative flex h-2 w-2">
            <span className="absolute inline-flex h-full w-full animate-ping rounded-full bg-blue-400 opacity-75" />
            <span className="relative inline-flex h-2 w-2 rounded-full bg-blue-500" />
          </span>
          {next.label}
        </span>
      );
    case "agent":
      return (
        <span className={clsx(base, schedulerOn ? "text-blue-600 dark:text-blue-400" : "text-blue-600/70 dark:text-blue-400/70")} title={title}>
          <Bot size={12} /> {schedulerOn ? next.label.replace("Next:", "Queued:") : next.label}
        </span>
      );
    case "waiting":
      return (
        <span className={clsx(base, "text-amber-600 dark:text-amber-400")} title={title}>
          <Clock size={11} /> {next.label}
          {until}
        </span>
      );
    case "human":
      return (
        <span className={clsx(base, "text-amber-700 dark:text-amber-300")} title={title}>
          <UserRound size={12} /> {next.label}
        </span>
      );
    case "blocked":
      if (next.action === "connect-claude") return <ConnectChip label={next.label} title={title} />;
      return (
        <span className={clsx(base, "text-rose-600 dark:text-rose-400")} title={title}>
          <Ban size={11} /> {next.label}
        </span>
      );
    default:
      return (
        <span className={clsx(base, "font-normal text-zinc-400 dark:text-zinc-500")} title={title}>
          <Moon size={11} /> {next.label}
        </span>
      );
  }
}

function CardView({ card, onOpen, dragging, schedulerOn, dim }: { card: CardT; onOpen?: () => void; dragging?: boolean; schedulerOn: boolean; dim?: boolean }) {
  const hasMeta = card.pr || card.comment_count > 0 || card.failure_count > 0 || card.last_run;
  return (
    <div
      onClick={onOpen}
      className={clsx(
        "group cursor-pointer rounded-md border border-l-4 bg-white dark:bg-zinc-900 p-2.5 shadow-xs hover:border-zinc-400 dark:hover:border-zinc-600 space-y-1.5 transition-opacity",
        card.hold === "needs_decision" ? "border-amber-400 dark:border-amber-700" : "border-zinc-200 dark:border-zinc-800",
        NEXT_ACCENT[card.next.kind],
        card.next.kind === "parked" && "bg-zinc-50 dark:bg-zinc-900/60",
        dragging && "shadow-lg rotate-1",
        dim && "opacity-35",
      )}
    >
      <div className="flex items-start gap-1.5">
        <span className="text-xs text-zinc-500 font-mono mt-0.5">#{card.number}</span>
        <span className={clsx("text-sm leading-snug flex-1", card.next.kind === "parked" && "text-zinc-600 dark:text-zinc-400")}>{card.title}</span>
        {card.priority && <Pill className={PRIORITY_STYLE[card.priority]}>{card.priority}</Pill>}
      </div>
      {(card.hold || card.labels.length > 0 || card.size) && (
        <div className="flex flex-wrap items-center gap-1">
          {card.hold && <HoldBadge hold={card.hold} reason={card.hold_reason} />}
          {card.labels.map((l) => (
            <LabelChip key={l.name} {...l} />
          ))}
          {card.size && <Pill className="bg-zinc-100 dark:bg-zinc-800 text-zinc-600 dark:text-zinc-400">{card.size}</Pill>}
        </div>
      )}
      {hasMeta && (
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
            <span className="text-rose-600" title={`${card.failure_count} failed run(s)`}>
              ✗{card.failure_count}
            </span>
          )}
          {card.last_run && card.next.kind !== "running" && <LastRun r={card.last_run} />}
        </div>
      )}
      <NextChip next={card.next} schedulerOn={schedulerOn} />
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

type CardProps = { schedulerOn: boolean; spotlight: Set<number> | null; onOpen: (n: number) => void };

function SortableCard({ card, schedulerOn, spotlight, onOpen }: CardProps & { card: CardT }) {
  const { attributes, listeners, setNodeRef, transform, transition, isDragging } = useSortable({ id: `card-${card.number}`, data: { card } });
  return (
    <div
      ref={setNodeRef}
      className="touch-manipulation select-none [-webkit-touch-callout:none]"
      style={{ transform: CSS.Transform.toString(transform), transition, opacity: isDragging ? 0.4 : 1 }}
      {...attributes}
      {...listeners}
    >
      <CardView card={card} onOpen={() => onOpen(card.number)} schedulerOn={schedulerOn} dim={!!spotlight && !spotlight.has(card.number)} />
    </div>
  );
}

type Section = { state: IssueState; title: string; hint: string; who: "agent" | "human" | "none" };

/** Sub-sections inside each column; each is a drop target that sets that state. */
const SECTIONS: Record<string, Section[]> = {
  backlog: [
    { state: "ready", title: "Ready", hint: "a fix agent picks these up", who: "agent" },
    { state: "triage", title: "Triage", hint: "a triage agent picks these up", who: "agent" },
    { state: "backlog", title: "Parked", hint: "agents never touch these", who: "none" },
  ],
  in_progress: [
    { state: "changes_requested", title: "Changes requested", hint: "the fix agent revises", who: "agent" },
    { state: "merge_conflict", title: "Merge conflict", hint: "the merge-prep agent resolves", who: "agent" },
    { state: "in_progress", title: "In progress", hint: "the fix agent works or resumes", who: "agent" },
  ],
  in_review: [
    { state: "ready_to_merge", title: "Ready to merge", hint: "waiting for you", who: "human" },
    { state: "in_review", title: "In review", hint: "a review agent picks these up", who: "agent" },
  ],
};

function SectionView({ section, cards, dragging, ...rest }: CardProps & { section: Section; cards: CardT[]; dragging: boolean }) {
  const { setNodeRef, isOver } = useDroppable({ id: `sec-${section.state}`, data: { state: section.state } });
  const Icon = section.who === "agent" ? Bot : section.who === "human" ? UserRound : Moon;
  // Empty sections only appear while dragging, as drop targets.
  if (cards.length === 0 && !dragging) return null;
  return (
    <div ref={setNodeRef} className={clsx("space-y-2 rounded-md", isOver && "bg-blue-50/70 dark:bg-blue-950/30 outline-2 outline-dashed outline-blue-400")}>
      <div className="flex items-center gap-1.5 px-1 pt-1 text-[11px] font-medium uppercase tracking-wide text-zinc-500">
        <Icon size={11} className={section.who === "agent" ? "text-blue-500" : section.who === "human" ? "text-amber-500" : ""} />
        {section.title}
        <span className="font-normal normal-case tracking-normal text-zinc-400">· {cards.length} · {section.hint}</span>
      </div>
      <SortableContext items={cards.map((c) => `card-${c.number}`)} strategy={verticalListSortingStrategy}>
        {cards.map((c) => (
          <SortableCard key={c.number} card={c} {...rest} />
        ))}
      </SortableContext>
      {cards.length === 0 && <div className="h-10 rounded border border-dashed border-zinc-300 dark:border-zinc-700" />}
    </div>
  );
}

function Column({ id, title, cards, dragging, ...rest }: CardProps & { id: string; title: string; cards: CardT[]; dragging: boolean }) {
  const { setNodeRef, isOver } = useDroppable({ id: `col-${id}`, data: { column: id } });
  const sections = SECTIONS[id];
  return (
    <div className="flex w-[85vw] max-w-80 shrink-0 snap-start flex-col rounded-lg bg-zinc-100/80 dark:bg-zinc-900/50 md:w-80">
      <div className="flex items-center gap-2 px-3 py-2 text-sm font-semibold">
        {title}
        <span className="rounded-full bg-zinc-200 dark:bg-zinc-800 px-1.5 text-xs font-normal text-zinc-600 dark:text-zinc-400">{cards.length}</span>
      </div>
      <div ref={sections ? undefined : setNodeRef} className={clsx("flex-1 space-y-3 overflow-y-auto px-2 pb-2 min-h-24", !sections && isOver && "bg-blue-50/60 dark:bg-blue-950/20 rounded-b-lg")}>
        {sections ? (
          sections.map((s) => <SectionView key={s.state} section={s} cards={cards.filter((c) => c.state === s.state)} dragging={dragging} {...rest} />)
        ) : (
          <SortableContext items={cards.map((c) => `card-${c.number}`)} strategy={verticalListSortingStrategy}>
            <div className="space-y-2">
              {cards.map((c) => (
                <SortableCard key={c.number} card={c} {...rest} />
              ))}
            </div>
          </SortableContext>
        )}
      </div>
    </div>
  );
}

function DispatchBanner({ board, spotlight, setSpotlight, onOpen }: { board: NonNullable<ReturnType<typeof useBoardData>>; spotlight: boolean; setSpotlight: (b: boolean) => void; onOpen: (n: number) => void }) {
  const cards = new Map(board.columns.flatMap((c) => c.cards).map((c) => [c.number, c]));
  const queued = board.dispatchable.map((n) => cards.get(n)).filter((c): c is CardT => !!c);
  if (queued.length === 0) {
    return (
      <div className="mx-4 mt-2 flex items-center gap-2 text-xs text-zinc-500">
        <Bot size={13} /> No issues are waiting for an agent{board.scheduler_enabled ? "." : ", so turning on the scheduler won't start anything."} Move issues to Triage or Ready to queue them.
      </div>
    );
  }
  const slots = Math.max(0, board.max_concurrent_runs - board.active_runs);
  return (
    <div
      className={clsx(
        "mx-4 mt-2 flex flex-wrap items-center gap-2 rounded-md border px-3 py-1.5 text-sm",
        board.scheduler_enabled ? "border-blue-200 bg-blue-50 dark:border-blue-900 dark:bg-blue-950/30" : "border-zinc-300 bg-white dark:border-zinc-700 dark:bg-zinc-900",
      )}
    >
      <Bot size={14} className="text-blue-600" />
      <span>
        {board.scheduler_enabled ? (
          <>
            <b>{queued.length}</b> issue{queued.length > 1 ? "s" : ""} queued for agents ({slots} free slot{slots === 1 ? "" : "s"}):
          </>
        ) : (
          <>
            Scheduler is off. Turning it on starts agents on <b>{queued.length}</b> issue{queued.length > 1 ? "s" : ""}
            {queued.length > board.max_concurrent_runs ? ` (${board.max_concurrent_runs} at a time)` : ""}:
          </>
        )}
      </span>
      {queued.slice(0, 8).map((c) => (
        <button key={c.number} onClick={() => onOpen(c.number)} className="rounded bg-blue-100 dark:bg-blue-900/50 px-1.5 text-xs text-blue-800 dark:text-blue-200 hover:underline" title={c.title}>
          #{c.number} {c.next.label.replace("Next: ", "")}
        </button>
      ))}
      {queued.length > 8 && <span className="text-xs text-zinc-500">+{queued.length - 8} more</span>}
      <label className="ml-auto flex items-center gap-1 text-xs text-zinc-500">
        <input type="checkbox" checked={spotlight} onChange={(e) => setSpotlight(e.target.checked)} /> highlight
      </label>
    </div>
  );
}

function useBoardData(q: ReturnType<typeof useQuery<Awaited<ReturnType<typeof fetchBoard>>>>) {
  return q.data;
}

async function fetchBoard(slug: string, filters: Record<string, unknown>) {
  return unwrap(client.GET("/api/projects/{p}/board", { params: { path: { p: slug }, query: filters } }));
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
  const [uploading, setUploading] = useState(false);
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
          <ImageTextarea slug={slug} className={clsx(inputCls, "font-mono h-40")} value={body} onChange={setBody} onPendingChange={setUploading} />
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
          <Button variant="primary" disabled={!title.trim() || create.isPending || uploading}>
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
    queryFn: () => fetchBoard(slug, filters),
    refetchInterval: 30000,
  });
  const [spotlightOn, setSpotlightOn] = useState(false);
  const spotlight = spotlightOn && board.data ? new Set(board.data.dispatchable) : null;
  const agents = useQuery({ queryKey: ["agents"], queryFn: () => unwrap(client.GET("/api/agents")) });
  // Mouse drags start after a small movement. On touch, a swipe scrolls and a long press picks the card up.
  const sensors = useSensors(
    useSensor(MouseSensor, { activationConstraint: { distance: 6 } }),
    useSensor(TouchSensor, { activationConstraint: { delay: 300, tolerance: 8 } }),
  );
  const [showFilters, setShowFilters] = useState(false);

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
    const column = (e.over.data.current?.column as string | undefined) ?? (overCard ? colOf.get(overCard.number) : undefined);
    // Dropping on a section (or a card in it) targets that section's state; the Done column asks.
    const target = (e.over.data.current?.state as IssueState | undefined) ?? (column === "done" ? undefined : overCard?.state);
    if (column === "done" || (!target && overCard && colOf.get(overCard.number) === "done")) {
      if (colOf.get(card.number) !== "done") setDrop({ card, column: "done" });
      return;
    }
    if (!target) return;
    if (target !== card.state) {
      transition.mutate({ n: card.number, to: target });
      return;
    }
    if (!overCard || overCard.number === card.number) return;
    const cards = board.data.columns.flatMap((c) => c.cards).filter((c) => c.state === target);
    const oldIdx = cards.findIndex((c) => c.number === card.number);
    const newIdx = cards.findIndex((c) => c.number === overCard.number);
    const without = cards.filter((c) => c.number !== card.number);
    const before = without[newIdx - 1];
    const after = without[newIdx];
    void oldIdx;
    const lo = before?.rank ?? (after ? after.rank - 2 : 0);
    const hi = after?.rank ?? lo + 2;
    rerank.mutate({ n: card.number, rank: (lo + hi) / 2 });
  };

  const activeFilters = [filters.label, filters.badge, filters.agent, filters.running].filter(Boolean).length;
  const needsConnect = board.data?.columns.flatMap((c) => c.cards).filter((c) => c.next.action === "connect-claude").length ?? 0;
  const openCard = (n: number) => nav(`/p/${slug}/issues/${n}${window.location.search}`);
  const labels = board.data?.labels ?? [];
  const pausedGroups = board.data?.limit_groups.filter((g) => g.paused) ?? [];

  return (
    <div className="flex h-full flex-col">
      <div className="flex flex-wrap items-center gap-2 border-b border-zinc-200 dark:border-zinc-800 px-3 py-2 md:px-4">
        <form
          onSubmit={(e) => {
            e.preventDefault();
            setFilter("q", q || undefined);
          }}
          className="relative min-w-0 flex-1 md:flex-none"
        >
          <Search size={13} className="absolute left-2 top-2 text-zinc-400" />
          <input className={clsx(fieldCls, "pl-7 w-full md:w-56 py-1")} placeholder="Search or #number" value={q} onChange={(e) => setQ(e.target.value)} />
        </form>
        <Button className="md:hidden" onClick={() => setShowFilters((v) => !v)} title="Filters">
          <SlidersHorizontal size={14} />
          {activeFilters > 0 && <span className="rounded-full bg-blue-600 px-1.5 text-[10px] leading-4 text-white">{activeFilters}</span>}
        </Button>
        <Button variant="primary" className="md:hidden" onClick={() => setNewOpen(true)} title="New issue">
          <Plus size={14} />
        </Button>
        <div className={clsx("flex w-full flex-wrap items-center gap-2 md:contents", !showFilters && "max-md:hidden")}>
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
        </div>
        {pausedGroups.map((g) => (
          <Pill key={g.name} className="bg-amber-100 text-amber-900 dark:bg-amber-950 dark:text-amber-300" title={g.pause_reason ?? ""}>
            <Clock size={11} /> {g.name} paused{g.paused_until ? ` · resumes ${timeAgo(g.paused_until)}` : ""}
          </Pill>
        ))}
        <div className="ml-auto hidden md:block">
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
      {board.data && needsConnect > 0 && <ConnectBanner count={needsConnect} />}
      {board.data && <DispatchBanner board={board.data} spotlight={spotlightOn} setSpotlight={setSpotlightOn} onOpen={openCard} />}
      <div className={clsx("flex min-h-0 flex-1 gap-3 overflow-x-auto p-3 md:p-4", !active && "max-md:snap-x max-md:snap-mandatory max-md:scroll-px-3")}>
        <DndContext sensors={sensors} collisionDetection={closestCorners} onDragStart={onDragStart} onDragEnd={onDragEnd} onDragCancel={() => setActive(null)}>
          {board.data?.columns.map((c) => (
            <Column
              key={c.id}
              id={c.id}
              title={c.title}
              cards={c.cards}
              dragging={!!active}
              schedulerOn={!!board.data?.scheduler_enabled}
              spotlight={spotlight}
              onOpen={openCard}
            />
          ))}
          <DragOverlay>{active ? <CardView card={active} dragging schedulerOn={!!board.data?.scheduler_enabled} /> : null}</DragOverlay>
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
