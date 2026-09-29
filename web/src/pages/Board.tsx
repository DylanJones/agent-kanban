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
import {
  AlertTriangle,
  Ban,
  Bot,
  Check,
  CheckCircle2,
  Clock,
  GitMerge,
  GitPullRequest,
  KeyRound,
  MessageSquare,
  MessagesSquare,
  Moon,
  PauseCircle,
  Plus,
  RotateCcw,
  Search,
  SlidersHorizontal,
  Square,
  UserRound,
  X,
  XCircle,
} from "lucide-react";
import { type ReactNode, useEffect, useMemo, useRef, useState } from "react";
import { Outlet, useNavigate, useParams, useSearchParams } from "react-router";
import { type Card as CardT, COLUMN_STATES, type IssueState, ROLE_LABEL, type Role, STATE_LABEL, api, client, unwrap } from "../api/client";
import { useConnectClaude } from "../components/ConnectClaude";
import { ImageTextarea } from "../components/ImageTextarea";
import { Button, ErrorBox, Field, HoldBadge, LabelChip, LiveDot, Modal, Segmented, StateBadge, Switch, TONE, inputCls, selectCls, timeAgo } from "../components/ui";

const PRIORITY_STYLE: Record<string, string> = {
  P0: "bg-rose-600 text-white",
  P1: "bg-orange-500/15 text-orange-700 dark:text-orange-300",
  P2: "bg-surface-3 text-fg-muted",
};

type Next = CardT["next"];

/** Each column's colour, used for its dot and the phone column tabs. */
const COLUMN_DOT: Record<string, string> = {
  backlog: "bg-fg-subtle",
  in_progress: "bg-sky-500",
  in_review: "bg-indigo-500",
  done: "bg-emerald-500",
};

function ConnectChip({ label, title }: { label: string; title: string }) {
  const connect = useConnectClaude();
  return (
    <button
      className={clsx("flex w-full items-center gap-1.5 rounded-lg px-2 py-1 text-left text-[11px] font-medium hover:underline", TONE.red)}
      title={title}
      onClick={(e) => {
        e.stopPropagation();
        connect();
      }}
      onPointerDown={(e) => e.stopPropagation()}
    >
      <KeyRound size={12} className="shrink-0" /> <span className="truncate">{label} · Connect</span>
    </button>
  );
}

function Banner({ tone, icon, children, action }: { tone: "accent" | "red" | "amber" | "neutral"; icon: ReactNode; children: ReactNode; action?: ReactNode }) {
  const styles = {
    accent: "border-accent/20 bg-accent-soft/60",
    red: "border-rose-500/25 bg-rose-500/8",
    amber: "border-amber-500/25 bg-amber-500/8",
    neutral: "border-line bg-surface-2/60",
  };
  const iconColor = { accent: "text-accent", red: "text-rose-600 dark:text-rose-400", amber: "text-amber-600 dark:text-amber-400", neutral: "text-fg-subtle" };
  return (
    <div className={clsx("flex flex-wrap items-center gap-x-3 gap-y-2 rounded-xl border px-3 py-2 text-sm", styles[tone])}>
      <span className={clsx("shrink-0", iconColor[tone])}>{icon}</span>
      <div className="min-w-0 flex-1">{children}</div>
      {action}
    </div>
  );
}

function ConnectBanner({ count }: { count: number }) {
  const connect = useConnectClaude();
  return (
    <Banner
      tone="red"
      icon={<KeyRound size={15} />}
      action={
        <Button size="sm" variant="primary" onClick={connect}>
          Connect Claude
        </Button>
      }
    >
      <span className="sm:hidden">
        <b>{count}</b> issue{count > 1 ? "s need" : " needs"} Claude connected for containers.
      </span>
      <span className="max-sm:hidden">
        <b>{count}</b> issue{count > 1 ? "s are" : " is"} waiting for Claude, which can't sign in inside containers until it's connected.
      </span>
    </Banner>
  );
}

function NextChip({ next, schedulerOn }: { next: Next; schedulerOn: boolean }) {
  if (next.kind === "done" || !next.label) return null;
  const until = next.until ? ` · ${timeAgo(next.until)}` : "";
  const title = [next.detail, next.kind === "agent" && !schedulerOn ? "The scheduler is off, so nothing starts until you turn it on or press Run." : ""]
    .filter(Boolean)
    .join(" ");
  const strip = "flex items-center gap-1.5 rounded-lg px-2 py-1 text-[11px] font-medium";
  const text = "flex items-center gap-1.5 px-0.5 text-[11px] font-medium";
  switch (next.kind) {
    case "running":
      return (
        <div className={clsx(strip, TONE.sky)} title={title}>
          <LiveDot tone="sky" /> <span className="truncate">{next.label}</span>
        </div>
      );
    case "agent":
      return (
        <div className={clsx(text, schedulerOn ? "text-accent-fg" : "text-fg-subtle")} title={title}>
          <Bot size={12} className="shrink-0" /> <span className="truncate">{schedulerOn ? next.label.replace("Next:", "Queued:") : next.label}</span>
        </div>
      );
    case "waiting":
      return (
        <div className={clsx(text, "text-amber-700 dark:text-amber-400")} title={title}>
          <Clock size={12} className="shrink-0" />{" "}
          <span className="truncate">
            {next.label}
            {until}
          </span>
        </div>
      );
    case "human":
      return (
        <div className={clsx(strip, TONE.amber)} title={title}>
          <UserRound size={12} className="shrink-0" /> <span className="truncate">{next.label}</span>
        </div>
      );
    case "blocked":
      if (next.action === "connect-claude") return <ConnectChip label={next.label} title={title} />;
      return (
        <div className={clsx(strip, TONE.red)} title={title}>
          <Ban size={12} className="shrink-0" /> <span className="truncate">{next.label}</span>
        </div>
      );
    default:
      return (
        <div className={clsx(text, "font-normal text-fg-subtle")} title={title}>
          <Moon size={11} className="shrink-0" /> <span className="truncate">{next.label}</span>
        </div>
      );
  }
}

function CardView({ card, onOpen, dragging, schedulerOn, dim }: { card: CardT; onOpen?: () => void; dragging?: boolean; schedulerOn: boolean; dim?: boolean }) {
  const pr = card.pr;
  const hasMeta = pr || card.comment_count > 0 || card.failure_count > 0 || card.last_run;
  const live = card.next.kind === "running";
  const parked = card.next.kind === "parked";
  return (
    <div
      onClick={onOpen}
      className={clsx(
        "group relative cursor-pointer overflow-hidden rounded-xl border bg-card p-3 shadow-card transition-[border-color,box-shadow,opacity,transform] duration-150 hover:border-line-strong hover:shadow-raised",
        card.hold === "needs_decision" ? "border-amber-500/50 ring-1 ring-amber-500/15" : live ? "border-sky-500/40 ring-1 ring-sky-500/15" : "border-line",
        dragging && "rotate-[1.5deg] shadow-overlay",
        dim && "opacity-35",
      )}
    >
      {live && <div className="live-bar absolute inset-x-0 top-0 h-[3px]" aria-hidden />}
      <div className="flex items-center gap-1.5 text-[11px] text-fg-subtle">
        <span className="font-mono tabular-nums">#{card.number}</span>
        <span className="ml-auto flex items-center gap-1">
          {card.size && <span className="rounded-md border border-line px-1.5 leading-4 font-medium">{card.size}</span>}
          {card.priority && <span className={clsx("rounded-md px-1.5 leading-4 font-semibold", PRIORITY_STYLE[card.priority])}>{card.priority}</span>}
        </span>
      </div>
      <div className={clsx("mt-1 line-clamp-4 text-[13.5px] leading-snug font-medium", parked ? "text-fg-muted" : "text-fg")}>{card.title}</div>
      {(card.hold || card.labels.length > 0) && (
        <div className="mt-2 flex flex-wrap items-center gap-1">
          {card.hold && <HoldBadge hold={card.hold} reason={card.hold_reason} />}
          {card.labels.map((l) => (
            <LabelChip key={l.name} {...l} />
          ))}
        </div>
      )}
      {hasMeta && (
        <div className="mt-2.5 flex items-center gap-3 text-[11px] text-fg-subtle">
          {pr && (
            <span
              className={clsx(
                "inline-flex items-center gap-1 font-medium",
                pr.state === "merged" ? "text-violet-600 dark:text-violet-400" : pr.has_conflicts ? "text-rose-600 dark:text-rose-400" : pr.approved ? "text-emerald-600 dark:text-emerald-400" : "text-fg-muted",
              )}
              title={pr.has_conflicts ? "Merge conflicts" : pr.approved ? "Approved at head" : `PR ${pr.state}`}
            >
              {pr.state === "merged" ? <GitMerge size={12} /> : <GitPullRequest size={12} />}#{pr.number}
              {pr.has_conflicts && <AlertTriangle size={11} />}
              {pr.approved && <Check size={11} strokeWidth={3} />}
            </span>
          )}
          {pr && pr.unresolved_threads > 0 && pr.state === "open" && (
            <span className="inline-flex items-center gap-1 text-amber-600 dark:text-amber-400" title="Unresolved review threads">
              <MessagesSquare size={11} />
              {pr.unresolved_threads}
            </span>
          )}
          {card.comment_count > 0 && (
            <span className="inline-flex items-center gap-1" title="Comments">
              <MessageSquare size={11} />
              {card.comment_count}
            </span>
          )}
          {card.failure_count > 0 && !card.hold && (
            <span className="inline-flex items-center gap-1 text-rose-600 dark:text-rose-400" title={`${card.failure_count} failed run(s)`}>
              <XCircle size={11} />
              {card.failure_count}
            </span>
          )}
          {card.last_run && card.next.kind !== "running" && <LastRun r={card.last_run} />}
        </div>
      )}
      <div className="mt-2 empty:hidden">
        <NextChip next={card.next} schedulerOn={schedulerOn} />
      </div>
    </div>
  );
}

const LAST_RUN_STYLE: Record<string, [typeof CheckCircle2, string]> = {
  succeeded: [CheckCircle2, "text-emerald-600 dark:text-emerald-400"],
  failed: [XCircle, "text-rose-600 dark:text-rose-400"],
  rate_limited: [PauseCircle, "text-amber-600 dark:text-amber-400"],
  cancelled: [Square, "text-fg-subtle"],
  interrupted: [RotateCcw, "text-fg-subtle"],
};

function LastRun({ r }: { r: NonNullable<CardT["last_run"]> }) {
  const age = r.ended_at ? (Date.now() - new Date(r.ended_at).getTime()) / 1000 : Infinity;
  if (age > 86400) return null;
  const [Icon, cls] = LAST_RUN_STYLE[r.status] ?? [Bot, "text-fg-subtle"];
  const verb = r.status === "succeeded" ? "done" : r.status.replace("_", " ");
  return (
    <span className={clsx("ml-auto inline-flex min-w-0 items-center gap-1", cls, age > 3600 && "opacity-70")} title={`Run #${r.id} ${r.status}${r.error ? `: ${r.error}` : ""}`}>
      <Icon size={11} className="shrink-0" />
      <span className="truncate">
        {ROLE_LABEL[r.role as Role] ?? r.role} {verb} · {timeAgo(r.ended_at)}
      </span>
    </span>
  );
}

type CardProps = { schedulerOn: boolean; spotlight: Set<number> | null; onOpen: (n: number) => void };

function SortableCard({ card, schedulerOn, spotlight, onOpen }: CardProps & { card: CardT }) {
  const { attributes, listeners, setNodeRef, transform, transition, isDragging } = useSortable({ id: `card-${card.number}`, data: { card } });
  return (
    <div
      ref={setNodeRef}
      className="touch-manipulation rounded-xl select-none [-webkit-touch-callout:none]"
      style={{ transform: CSS.Transform.toString(transform), transition, opacity: isDragging ? 0.4 : 1 }}
      {...attributes}
      {...listeners}
      aria-label={`#${card.number} ${card.title}`}
      onKeyDown={(e) => {
        if (e.key === "Enter" || e.key === " ") {
          e.preventDefault();
          onOpen(card.number);
        }
      }}
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
    <div ref={setNodeRef} className={clsx("space-y-2 rounded-xl transition-colors", isOver && "bg-accent-soft/70 outline-2 outline-offset-2 outline-accent/50 outline-dashed")}>
      <div className="flex items-center gap-1.5 px-1 pt-0.5 text-[11px] font-semibold text-fg-muted" title={section.hint}>
        <Icon size={12} className={clsx("shrink-0", section.who === "agent" ? "text-accent" : section.who === "human" ? "text-amber-500" : "text-fg-subtle")} />
        <span className="shrink-0 whitespace-nowrap">{section.title}</span>
        <span className="shrink-0 font-medium text-fg-subtle tabular-nums">{cards.length}</span>
        <span className="truncate font-normal text-fg-subtle">· {section.hint}</span>
      </div>
      <SortableContext items={cards.map((c) => `card-${c.number}`)} strategy={verticalListSortingStrategy}>
        {cards.map((c) => (
          <SortableCard key={c.number} card={c} {...rest} />
        ))}
      </SortableContext>
      {cards.length === 0 && <div className="h-12 rounded-xl border border-dashed border-line-strong" />}
    </div>
  );
}

function Column({ id, title, cards, dragging, ...rest }: CardProps & { id: string; title: string; cards: CardT[]; dragging: boolean }) {
  const { setNodeRef, isOver } = useDroppable({ id: `col-${id}`, data: { column: id } });
  const sections = SECTIONS[id];
  return (
    <div className="flex w-full shrink-0 snap-center snap-always flex-col md:w-[300px] md:rounded-2xl md:border md:border-line/70 md:bg-lane lg:w-auto lg:max-w-[380px] lg:min-w-[272px] lg:flex-1 lg:shrink">
      <div className="flex items-center gap-2 px-3.5 pt-3 pb-2 max-md:hidden">
        <span className={clsx("h-2 w-2 rounded-full", COLUMN_DOT[id] ?? "bg-fg-subtle")} />
        <h2 className="text-[13px] font-semibold tracking-tight">{title}</h2>
        <span className="text-xs text-fg-subtle tabular-nums">{cards.length}</span>
      </div>
      <div
        ref={sections ? undefined : setNodeRef}
        className={clsx(
          "min-h-24 flex-1 space-y-4 overflow-y-auto px-4 pt-1 pb-24 md:px-2 md:pb-2",
          !sections && isOver && "rounded-b-2xl bg-accent-soft/60",
        )}
      >
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
        {cards.length === 0 && !dragging && <div className="py-10 text-center text-xs text-fg-subtle">Nothing here</div>}
      </div>
    </div>
  );
}

function DispatchBanner({ board, spotlight, setSpotlight, onOpen }: { board: NonNullable<ReturnType<typeof useBoardData>>; spotlight: boolean; setSpotlight: (b: boolean) => void; onOpen: (n: number) => void }) {
  const cards = new Map(board.columns.flatMap((c) => c.cards).map((c) => [c.number, c]));
  const queued = board.dispatchable.map((n) => cards.get(n)).filter((c): c is CardT => !!c);
  if (queued.length === 0) {
    return (
      <div className="flex items-center gap-2 px-1 text-xs text-fg-subtle max-md:hidden">
        <Bot size={13} className="shrink-0" /> No issues are waiting for an agent{board.scheduler_enabled ? "." : ", so turning on the scheduler won't start anything."} Move issues to Triage or Ready to queue them.
      </div>
    );
  }
  const slots = Math.max(0, board.max_concurrent_runs - board.active_runs);
  return (
    <Banner
      tone={board.scheduler_enabled ? "accent" : "neutral"}
      icon={<Bot size={15} />}
      action={
        <label className="flex items-center gap-2 text-xs text-fg-muted">
          Highlight <Switch size="sm" checked={spotlight} onChange={setSpotlight} label="Highlight queued issues" />
        </label>
      }
    >
      <div className="flex flex-wrap items-center gap-x-2 gap-y-1.5">
        <span>
          {board.scheduler_enabled ? (
            <>
              <b>{queued.length}</b> issue{queued.length > 1 ? "s" : ""} queued for agents ({slots} free slot{slots === 1 ? "" : "s"})
            </>
          ) : (
            <>
              Scheduler is off. Turning it on starts agents on <b>{queued.length}</b> issue{queued.length > 1 ? "s" : ""}
              {queued.length > board.max_concurrent_runs ? ` (${board.max_concurrent_runs} at a time)` : ""}
            </>
          )}
        </span>
        {queued.slice(0, 8).map((c) => (
          <button
            key={c.number}
            onClick={() => onOpen(c.number)}
            className="rounded-md bg-surface/80 px-1.5 py-0.5 text-xs font-medium text-accent-fg ring-1 ring-accent/20 hover:bg-surface"
            title={c.title}
          >
            #{c.number} {c.next.label.replace("Next: ", "")}
          </button>
        ))}
        {queued.length > 8 && <span className="text-xs text-fg-subtle">+{queued.length - 8} more</span>}
      </div>
    </Banner>
  );
}

function useBoardData(q: ReturnType<typeof useQuery<Awaited<ReturnType<typeof fetchBoard>>>>) {
  return q.data;
}

async function fetchBoard(slug: string, filters: Record<string, unknown>) {
  return unwrap(client.GET("/api/projects/{p}/board", { params: { path: { p: slug }, query: filters } }));
}

type Drop = { card: CardT; column: string };

const STATE_HINT: Partial<Record<IssueState, string>> = {
  triage: "A triage agent will look at it",
  ready: "A fix agent will pick it up",
  backlog: "Park it; no agent will touch it",
  done: "Mark completed (cleans up worktree)",
};

function StatePicker({ drop, onClose, onPick }: { drop: Drop | null; onClose: () => void; onPick: (s: IssueState, reason?: string) => void }) {
  const [reason, setReason] = useState("wontfix");
  if (!drop) return null;
  const states = COLUMN_STATES[drop.column].filter((s) => s !== drop.card.state);
  return (
    <Modal open onClose={onClose} title={`Move #${drop.card.number} to…`}>
      <div className="space-y-2">
        {states.map((s) =>
          s === "closed" ? (
            <div key={s} className="flex flex-col gap-2 rounded-xl border border-line p-3 sm:flex-row sm:items-center">
              <span className="text-sm font-medium sm:flex-1">Close without completing</span>
              <select className={clsx(inputCls, selectCls, "sm:w-40")} value={reason} onChange={(e) => setReason(e.target.value)} aria-label="Close reason">
                {["wontfix", "duplicate", "invalid", "not_planned"].map((r) => (
                  <option key={r}>{r}</option>
                ))}
              </select>
              <Button onClick={() => onPick("closed", reason)} className="justify-center">
                Close as {reason}
              </Button>
            </div>
          ) : (
            <button
              key={s}
              onClick={() => onPick(s)}
              className="flex w-full items-center gap-3 rounded-xl border border-line bg-surface p-3 text-left transition-colors hover:border-line-strong hover:bg-surface-2"
            >
              <StateBadge state={s} always />
              <span className="text-sm text-fg-muted">{STATE_HINT[s]}</span>
            </button>
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
        className="space-y-4"
        onSubmit={(e) => {
          e.preventDefault();
          create.mutate();
        }}
      >
        <input
          autoFocus
          aria-label="Title"
          className="w-full bg-transparent text-lg font-semibold tracking-tight outline-none placeholder:text-fg-subtle"
          placeholder="Issue title"
          value={title}
          onChange={(e) => setTitle(e.target.value)}
        />
        <ImageTextarea
          slug={slug}
          className={clsx(inputCls, "h-40 resize-y font-mono sm:text-[13px]")}
          placeholder="Describe the problem (markdown). Paste or drop screenshots."
          value={body}
          onChange={setBody}
          onPendingChange={setUploading}
        />
        <div className="grid gap-4 sm:grid-cols-[1fr_auto_auto]">
          <Field label="Labels (comma-separated)">
            <input className={inputCls} value={labels} onChange={(e) => setLabels(e.target.value)} />
          </Field>
          <div className="space-y-1.5">
            <span className="block text-xs font-medium text-fg-muted">Priority</span>
            <Segmented
              label="Priority"
              value={priority}
              onChange={setPriority}
              options={[
                { value: "", label: "None" },
                { value: "P0", label: "P0" },
                { value: "P1", label: "P1" },
                { value: "P2", label: "P2" },
              ]}
            />
          </div>
          <div className="space-y-1.5">
            <span className="block text-xs font-medium text-fg-muted">Start in</span>
            <Segmented label="Start in" value={state} onChange={setState} options={(["triage", "backlog", "ready"] as IssueState[]).map((s) => ({ value: s, label: STATE_LABEL[s] }))} />
          </div>
        </div>
        <ErrorBox error={create.error} />
        <div className="flex items-center justify-end gap-2 border-t border-line pt-4">
          <Button type="button" variant="ghost" onClick={onClose}>
            Cancel
          </Button>
          <Button variant="primary" disabled={!title.trim() || create.isPending || uploading}>
            Create issue
          </Button>
        </div>
      </form>
    </Modal>
  );
}

const BADGES = ["triage", "ready", "changes_requested", "merge_conflict", "ready_to_merge", "needs_decision", "stalled", "paused", "closed"];

function FilterSelect({ value, onChange, children, label }: { value?: string; onChange: (v?: string) => void; children: ReactNode; label: string }) {
  return (
    <select
      aria-label={label}
      className={clsx(
        selectCls,
        "h-8 max-w-44 shrink-0 rounded-lg border pl-3 text-base font-medium shadow-card outline-none transition-colors sm:text-[13px]",
        value ? "border-accent/30 bg-accent-soft text-accent-fg" : "border-line bg-surface text-fg-muted hover:text-fg",
      )}
      value={value ?? ""}
      onChange={(e) => onChange(e.target.value || undefined)}
    >
      {children}
    </select>
  );
}

function BoardSkeleton() {
  return (
    <>
      {[3, 2, 4, 3].map((n, i) => (
        <div key={i} className="flex w-full shrink-0 flex-col gap-2 px-4 max-md:[&:not(:first-child)]:hidden md:w-[300px] md:rounded-2xl md:border md:border-line/70 md:bg-lane md:p-2 lg:w-auto lg:max-w-[380px] lg:min-w-[272px] lg:flex-1">
          <div className="mx-1.5 my-2 h-3 w-24 animate-pulse rounded-full bg-surface-3 max-md:hidden" />
          {Array.from({ length: n }, (_, j) => (
            <div key={j} className="space-y-2.5 rounded-xl border border-line bg-card p-3">
              <div className="h-2.5 w-10 animate-pulse rounded-full bg-surface-3" />
              <div className="h-3 w-11/12 animate-pulse rounded-full bg-surface-3" />
              <div className="h-3 w-2/3 animate-pulse rounded-full bg-surface-3" />
            </div>
          ))}
        </div>
      ))}
    </>
  );
}

function isTyping(el: EventTarget | null) {
  if (!(el instanceof HTMLElement)) return false;
  return el.isContentEditable || ["INPUT", "TEXTAREA", "SELECT"].includes(el.tagName);
}

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
  const [filtersOpen, setFiltersOpen] = useState(false);
  const [activeCol, setActiveCol] = useState(0);
  const scroller = useRef<HTMLDivElement>(null);
  const search = useRef<HTMLInputElement>(null);
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

  // Desktop shortcuts: "/" focuses search, "c" starts a new issue.
  useEffect(() => {
    const k = (e: KeyboardEvent) => {
      if (e.metaKey || e.ctrlKey || e.altKey || isTyping(e.target) || document.querySelector('[aria-modal="true"]')) return;
      if (e.key === "/") {
        e.preventDefault();
        search.current?.focus();
      } else if (e.key === "c") {
        e.preventDefault();
        setNewOpen(true);
      }
    };
    window.addEventListener("keydown", k);
    return () => window.removeEventListener("keydown", k);
  }, []);

  const setFilter = (k: string, v?: string) => {
    const next = new URLSearchParams(sp);
    if (v) next.set(k, v);
    else next.delete(k);
    setSp(next, { replace: true });
  };
  const clearFilters = () => {
    const next = new URLSearchParams(sp);
    for (const k of ["label", "badge", "agent", "running", "q"]) next.delete(k);
    setQ("");
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
  const allCards = board.data?.columns.flatMap((c) => c.cards) ?? [];
  const needsConnect = allCards.filter((c) => c.next.action === "connect-claude").length;
  const openCard = (n: number) => nav(`/p/${slug}/issues/${n}${window.location.search}`);
  const labels = board.data?.labels ?? [];
  const pausedGroups = board.data?.limit_groups.filter((g) => g.paused) ?? [];
  const running = allCards.filter((c) => c.next.kind === "running").length;
  const columns = board.data?.columns ?? [];

  const filterControls = (
    <>
      <FilterSelect label="Label" value={filters.label} onChange={(v) => setFilter("label", v)}>
        <option value="">All labels</option>
        {labels.map((l) => (
          <option key={l.name}>{l.name}</option>
        ))}
      </FilterSelect>
      <FilterSelect label="State" value={filters.badge} onChange={(v) => setFilter("badge", v)}>
        <option value="">All states</option>
        {BADGES.map((b) => (
          <option key={b} value={b}>
            {b.charAt(0).toUpperCase() + b.slice(1).replace(/_/g, " ")}
          </option>
        ))}
      </FilterSelect>
      <FilterSelect label="Agent" value={filters.agent} onChange={(v) => setFilter("agent", v)}>
        <option value="">Any agent</option>
        {agents.data?.map((a) => (
          <option key={a.slug} value={a.slug}>
            {a.name}
          </option>
        ))}
      </FilterSelect>
      <button
        type="button"
        aria-pressed={!!filters.running}
        onClick={() => setFilter("running", filters.running ? undefined : "1")}
        className={clsx(
          "inline-flex h-8 shrink-0 items-center gap-1.5 rounded-lg border px-3 text-[13px] font-medium shadow-card transition-colors",
          filters.running ? "border-sky-500/30 bg-sky-500/12 text-sky-700 dark:text-sky-300" : "border-line bg-surface text-fg-muted hover:text-fg",
        )}
      >
        {filters.running ? <LiveDot tone="sky" /> : <span className="h-2 w-2 rounded-full border border-current" />}
        Running
      </button>
    </>
  );

  const selectColumn = (i: number) => {
    const el = scroller.current;
    if (el) el.scrollTo({ left: i * el.clientWidth, behavior: "smooth" });
    setActiveCol(i);
  };

  return (
    <div className="flex h-full flex-col">
      {/* Toolbar */}
      <div className="flex flex-wrap items-center gap-2 px-4 pt-3 pb-3 md:gap-3 md:px-6 md:pt-5">
        <div className="mr-2 hidden min-w-0 items-baseline gap-2 md:flex">
          <h1 className="text-lg font-semibold tracking-tight">Board</h1>
          {board.data && (
            <span className="text-sm whitespace-nowrap text-fg-subtle tabular-nums">
              {allCards.length} issues{running > 0 ? ` · ${running} running` : ""}
            </span>
          )}
        </div>
        <form
          onSubmit={(e) => {
            e.preventDefault();
            setFilter("q", q || undefined);
          }}
          className="relative min-w-0 flex-1 md:max-w-64 md:flex-none"
        >
          <Search size={15} className="pointer-events-none absolute top-1/2 left-3 -translate-y-1/2 text-fg-subtle" />
          <input
            ref={search}
            type="search"
            className="h-9 w-full rounded-lg border border-line bg-surface pr-8 pl-9 text-base shadow-card outline-none placeholder:text-fg-subtle focus:border-accent/60 focus:ring-3 focus:ring-accent/15 md:h-8 md:text-[13px]"
            placeholder="Search or #number"
            value={q}
            onChange={(e) => setQ(e.target.value)}
          />
          {!q && <kbd className="pointer-events-none absolute top-1/2 right-2 hidden -translate-y-1/2 rounded border border-line px-1.5 font-mono text-[10px] text-fg-subtle md:block">/</kbd>}
        </form>
        <Button className="relative md:hidden" icon onClick={() => setFiltersOpen(true)} aria-label="Filters" title="Filters">
          <SlidersHorizontal size={16} />
          {activeFilters > 0 && <span className="absolute -top-1 -right-1 grid h-4 min-w-4 place-items-center rounded-full bg-accent px-1 text-[10px] leading-none text-white">{activeFilters}</span>}
        </Button>
        <div className="hidden flex-wrap items-center gap-2 md:flex">{filterControls}</div>
        {(activeFilters > 0 || filters.q) && (
          <Button variant="ghost" size="sm" onClick={clearFilters} className="max-md:hidden">
            <X size={13} /> Clear
          </Button>
        )}
        <div className="ml-auto hidden md:block">
          <Button variant="primary" onClick={() => setNewOpen(true)} title="New issue (c)">
            <Plus size={16} /> New issue
          </Button>
        </div>
      </div>

      {/* Banners */}
      {board.data && (
        <div className={clsx("space-y-2 px-4 pb-3 md:px-6", !moveError && needsConnect === 0 && pausedGroups.length === 0 && board.data.dispatchable.length === 0 && "max-md:hidden")}>
          {moveError ? (
            <div onClick={() => setMoveError(null)}>
              <ErrorBox error={moveError} />
            </div>
          ) : null}
          {needsConnect > 0 && <ConnectBanner count={needsConnect} />}
          {pausedGroups.length > 0 && (
            <Banner tone="amber" icon={<Clock size={15} />}>
              {pausedGroups.map((g) => (
                <span key={g.name} className="mr-3 inline-block" title={g.pause_reason ?? ""}>
                  <b>{g.name}</b> is paused{g.paused_until ? ` · resumes ${timeAgo(g.paused_until)}` : ""}
                </span>
              ))}
            </Banner>
          )}
          <DispatchBanner board={board.data} spotlight={spotlightOn} setSpotlight={setSpotlightOn} onOpen={openCard} />
        </div>
      )}
      <div className="px-4 md:px-6">
        <ErrorBox error={board.error} />
      </div>

      {/* Phone column tabs */}
      {columns.length > 0 && (
        <div className="px-4 pb-3 md:hidden">
          <div className="grid rounded-xl bg-surface-3/70 p-1" style={{ gridTemplateColumns: `repeat(${columns.length}, minmax(0, 1fr))` }} role="tablist" aria-label="Columns">
            {columns.map((c, i) => (
              <button
                key={c.id}
                role="tab"
                aria-selected={activeCol === i}
                onClick={() => selectColumn(i)}
                className={clsx(
                  "flex min-w-0 flex-col items-center rounded-lg px-1 py-1.5 transition-colors",
                  activeCol === i ? "bg-surface text-fg shadow-card" : "text-fg-muted",
                )}
              >
                <span className="flex max-w-full items-center gap-1 text-[12px] font-medium">
                  <span className={clsx("h-1.5 w-1.5 shrink-0 rounded-full", COLUMN_DOT[c.id])} />
                  <span className="truncate">{c.title}</span>
                </span>
                <span className="text-[11px] text-fg-subtle tabular-nums">{c.cards.length}</span>
              </button>
            ))}
          </div>
        </div>
      )}

      {/* Columns */}
      <div
        ref={scroller}
        onScroll={(e) => {
          const el = e.currentTarget;
          if (el.clientWidth && window.matchMedia("(max-width: 767px)").matches) setActiveCol(Math.round(el.scrollLeft / el.clientWidth));
        }}
        className={clsx("flex min-h-0 flex-1 overflow-x-auto max-md:no-scrollbar md:gap-4 md:px-6 md:pb-6", !active && "max-md:snap-x max-md:snap-mandatory")}
      >
        {board.isLoading && <BoardSkeleton />}
        <DndContext sensors={sensors} collisionDetection={closestCorners} onDragStart={onDragStart} onDragEnd={onDragEnd} onDragCancel={() => setActive(null)}>
          {columns.map((c) => (
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

      {/* Phone: new issue */}
      <button
        onClick={() => setNewOpen(true)}
        className="fixed right-4 bottom-[calc(4.75rem+env(safe-area-inset-bottom))] z-30 grid h-14 w-14 place-items-center rounded-2xl bg-accent text-white shadow-overlay inset-shadow-[inset_0_1px_0_rgb(255_255_255/0.2)] transition-transform active:scale-95 md:hidden"
        aria-label="New issue"
      >
        <Plus size={26} />
      </button>

      <Modal open={filtersOpen} onClose={() => setFiltersOpen(false)} title="Filters">
        <div className="space-y-4">
          <div className="grid grid-cols-1 gap-3 [&>select]:h-11 [&>select]:max-w-none [&>button]:h-11">{filterControls}</div>
          <div className="flex gap-2">
            <Button
              className="flex-1 justify-center"
              onClick={() => {
                clearFilters();
                setFiltersOpen(false);
              }}
              disabled={activeFilters === 0 && !filters.q}
            >
              Clear all
            </Button>
            <Button variant="primary" className="flex-1 justify-center" onClick={() => setFiltersOpen(false)}>
              Done
            </Button>
          </div>
        </div>
      </Modal>
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
