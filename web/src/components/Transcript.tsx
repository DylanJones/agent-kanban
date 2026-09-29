import { useMutation, useQueryClient } from "@tanstack/react-query";
import clsx from "clsx";
import { Brain, CheckCircle2, ChevronDown, ChevronRight, Circle, CircleDot, FileText, Loader2, MessageSquare, Pencil, ScrollText, Search, ShieldAlert, Terminal, Undo2, Wrench, XCircle } from "lucide-react";
import { type ReactNode, useState } from "react";
import { type RunEvent, api } from "../api/client";
import { Avatar, Button, Markdown } from "./ui";

type Payload = Record<string, unknown>;

// ---------- shell highlighting ----------

const TOKEN = /(#[^\n]*)|('(?:[^'\\]|\\.)*'|"(?:[^"\\]|\\.)*")|(\$\{[^}]+\}|\$[A-Za-z_][A-Za-z0-9_]*|\$\()|(&&|\|\||;;|[|;&<>]+)|(\s--?[A-Za-z0-9][\w-]*=?)|(\n)|(\s+)|([^\s'"$|;&<>#]+)/g;

export function ShellHighlight({ code }: { code: string }) {
  const out: ReactNode[] = [];
  let expectCmd = true;
  let m: RegExpExecArray | null;
  let i = 0;
  TOKEN.lastIndex = 0;
  while ((m = TOKEN.exec(code))) {
    const [t, comment, str, variable, op, flag, nl, ws, word] = m;
    const k = i++;
    if (comment) out.push(<span key={k} className="text-fg-subtle italic">{t}</span>);
    else if (str) {
      out.push(<span key={k} className="text-emerald-700 dark:text-emerald-400">{t}</span>);
      expectCmd = false;
    } else if (variable) out.push(<span key={k} className="text-violet-700 dark:text-violet-400">{t}</span>);
    else if (op) {
      out.push(<span key={k} className="text-rose-600 dark:text-rose-400">{t}</span>);
      expectCmd = !t.includes(">") && !t.includes("<");
    } else if (flag) out.push(<span key={k} className="text-sky-700 dark:text-sky-400">{t}</span>);
    else if (nl) {
      out.push(t);
      expectCmd = true;
    } else if (ws) out.push(t);
    else if (word) {
      if (expectCmd && !/^\w+=/.test(word)) {
        out.push(<span key={k} className="font-semibold text-amber-700 dark:text-amber-300">{t}</span>);
        expectCmd = ["sudo", "env", "time", "nohup", "xargs", "cd"].includes(word) ? word !== "cd" : false;
      } else out.push(t);
    } else out.push(t);
  }
  return <>{out}</>;
}

// ---------- tool call helpers ----------

function commandOf(p: Payload): string | undefined {
  const raw = p.rawInput as Payload | undefined;
  const c = raw?.command ?? raw?.cmd;
  if (typeof c === "string") return c;
  if (Array.isArray(c)) {
    const parts = c.map(String);
    if (parts.length === 3 && parts[0].endsWith("sh") && parts[1].startsWith("-")) return parts[2];
    return parts.join(" ");
  }
  return undefined;
}

function pathOf(p: Payload): string | undefined {
  const locs = p.locations as { path?: string; line?: number }[] | undefined;
  const raw = p.rawInput as Payload | undefined;
  return locs?.[0]?.path ?? (raw?.file_path as string) ?? (raw?.path as string) ?? undefined;
}

function shortPath(path: string, worktree?: string | null) {
  if (worktree && path.startsWith(worktree)) return path.slice(worktree.length).replace(/^\//, "");
  return path;
}

function kindOf(p: Payload): string {
  const k = String(p.kind ?? "other");
  if (k === "other" && commandOf(p)) return "execute";
  return k;
}

function summary(p: Payload, worktree?: string | null): string {
  const raw = p.rawInput as Payload | undefined;
  const cmd = commandOf(p);
  if (cmd) {
    const d = raw?.description;
    if (typeof d === "string" && d.trim()) return d;
    let line = cmd.split("\n")[0].replace(/^\s*cd\s+\S+\s*(&&|;)\s*/, "");
    if (worktree) line = line.split(worktree + "/").join("").split(worktree).join(".");
    return line.slice(0, 160);
  }
  const path = pathOf(p);
  const title = String(p.title ?? p.kind ?? "tool");
  if (path && !title.includes(shortPath(path, worktree))) return `${title} ${shortPath(path, worktree)}`;
  return title.replace(worktree ?? "\u0000", "").slice(0, 160);
}

function stripFences(s: string) {
  return s.replace(/^```[a-z]*\n?/, "").replace(/\n?```\s*$/, "");
}

function outputOf(p: Payload): string | undefined {
  const content = (p.content as Payload[] | undefined) ?? [];
  const texts: string[] = [];
  for (const c of content) {
    const inner = c.content as Payload | undefined;
    if (c.type === "content" && inner?.type === "text") texts.push(stripFences(String(inner.text)));
    if (c.type === "terminal") texts.push(`[terminal ${String(c.terminalId)}]`);
  }
  if (texts.length) return texts.join("\n");
  const ro = p.rawOutput;
  if (ro === undefined || ro === null) return undefined;
  if (typeof ro === "string") return stripFences(ro);
  const o = ro as Payload;
  if (typeof o.stdout === "string" || typeof o.stderr === "string") return [o.stdout, o.stderr].filter(Boolean).join("\n");
  if (typeof o.output === "string") return o.output;
  if (typeof o.formatted_output === "string") return o.formatted_output;
  return JSON.stringify(ro, null, 2);
}

function diffsOf(p: Payload): { path: string; oldText?: string; newText: string }[] {
  return ((p.content as Payload[] | undefined) ?? [])
    .filter((c) => c.type === "diff")
    .map((c) => ({ path: String(c.path), oldText: c.oldText as string | undefined, newText: String(c.newText ?? "") }));
}

function DiffBlock({ oldText, newText }: { oldText?: string; newText: string }) {
  const oldLines = oldText ? oldText.split("\n") : [];
  const newLines = newText.split("\n");
  // Trim the common prefix/suffix so only the changed region shows.
  let a = 0;
  while (a < oldLines.length && a < newLines.length && oldLines[a] === newLines[a]) a++;
  let b = 0;
  while (b < oldLines.length - a && b < newLines.length - a && oldLines[oldLines.length - 1 - b] === newLines[newLines.length - 1 - b]) b++;
  const ctx = 2;
  const pre = oldLines.slice(Math.max(0, a - ctx), a);
  const removed = oldLines.slice(a, oldLines.length - b);
  const added = newLines.slice(a, newLines.length - b);
  const post = newLines.slice(newLines.length - b, Math.min(newLines.length, newLines.length - b + ctx));
  return (
    <pre className="max-h-80 overflow-auto rounded-lg border border-line bg-surface py-1 font-mono text-[11px] leading-[1.15rem]">
      {pre.map((l, i) => <div key={`p${i}`} className="px-2.5 text-fg-subtle">  {l}</div>)}
      {removed.map((l, i) => <div key={`r${i}`} className="bg-rose-500/12 px-2.5 text-rose-900 dark:text-rose-200">- {l}</div>)}
      {added.map((l, i) => <div key={`a${i}`} className="bg-emerald-500/12 px-2.5 text-emerald-900 dark:text-emerald-200">+ {l}</div>)}
      {post.map((l, i) => <div key={`s${i}`} className="px-2.5 text-fg-subtle">  {l}</div>)}
    </pre>
  );
}

function StatusIcon({ status }: { status: string }) {
  if (status === "completed") return <CheckCircle2 size={13} className="shrink-0 text-emerald-500" />;
  if (status === "failed") return <XCircle size={13} className="shrink-0 text-rose-500" />;
  if (status === "in_progress") return <Loader2 size={13} className="shrink-0 animate-spin text-sky-500" />;
  return <Circle size={13} className="shrink-0 text-fg-subtle" />;
}

function KindIcon({ kind }: { kind: string }) {
  const cls = "shrink-0 text-fg-subtle";
  if (kind === "execute") return <Terminal size={12} className={cls} />;
  if (kind === "read") return <FileText size={12} className={cls} />;
  if (kind === "edit" || kind === "delete" || kind === "move") return <Pencil size={12} className={cls} />;
  if (kind === "search") return <Search size={12} className={cls} />;
  return <Wrench size={12} className={cls} />;
}

type ToolItem = { ev: RunEvent; perm?: Payload };

function ToolRow({ item, worktree }: { item: ToolItem; worktree?: string | null }) {
  const [open, setOpen] = useState(false);
  const [showAll, setShowAll] = useState(false);
  const p = item.ev.payload as Payload;
  const kind = kindOf(p);
  const cmd = commandOf(p);
  const out = open ? outputOf(p) : undefined;
  const diffs = open ? diffsOf(p) : [];
  const outLines = out?.split("\n") ?? [];
  const perm = item.perm;
  return (
    <div>
      <button onClick={() => setOpen(!open)} className="group flex w-full items-center gap-2 rounded-md px-1.5 py-1 text-left text-xs hover:bg-surface-2">
        {open ? <ChevronDown size={12} className="shrink-0 text-fg-subtle" /> : <ChevronRight size={12} className="shrink-0 text-fg-subtle" />}
        <StatusIcon status={String(p.status ?? "pending")} />
        <KindIcon kind={kind} />
        <span className={clsx("truncate", cmd && !(p.rawInput as Payload)?.description ? "font-mono" : "", "text-fg-muted group-hover:text-fg")}>{summary(p, worktree)}</span>
        {perm && typeof perm.decision === "string" && perm.decision.startsWith("denied") && (
          <span className="ml-auto shrink-0 rounded-md bg-rose-500/12 px-1.5 text-[10px] font-medium text-rose-700 dark:text-rose-300">{perm.decision}</span>
        )}
      </button>
      {open && (
        <div className="mt-1 mb-2.5 ml-7 space-y-1.5">
          {cmd && (
            <pre className="overflow-x-auto rounded-lg border border-line bg-surface px-3 py-2 font-mono text-[11px] leading-[1.15rem] break-all whitespace-pre-wrap">
              <span className="text-fg-subtle select-none">$ </span>
              <ShellHighlight code={cmd} />
            </pre>
          )}
          {!cmd && pathOf(p) && <div className="font-mono text-[11px] break-all text-fg-subtle">{pathOf(p)}</div>}
          {diffs.map((d, i) => (
            <div key={i}>
              <div className="mb-1 font-mono text-[11px] break-all text-fg-subtle">{shortPath(d.path, worktree)}</div>
              <DiffBlock oldText={d.oldText} newText={d.newText} />
            </div>
          ))}
          {out !== undefined && out.trim() !== "" && diffs.length === 0 && (
            <div className="relative">
              <pre className={clsx("overflow-auto rounded-lg bg-surface-3/60 px-3 py-2 font-mono text-[11px] leading-[1.15rem] break-all whitespace-pre-wrap text-fg-muted", !showAll && "max-h-64")}>
                {showAll ? out : outLines.slice(0, 200).join("\n")}
              </pre>
              {(outLines.length > 200 || (!showAll && outLines.length > 18)) && (
                <button onClick={() => setShowAll(!showAll)} className="mt-1 text-[11px] font-medium text-accent-fg hover:underline">
                  {showAll ? "Collapse output" : `Show all ${outLines.length} lines`}
                </button>
              )}
            </div>
          )}
          {perm && (
            <div className="flex items-center gap-1 text-[10px] text-fg-subtle">
              <ShieldAlert size={11} /> {String(perm.decision ?? perm.status)}
            </div>
          )}
        </div>
      )}
    </div>
  );
}

function groupLabel(items: ToolItem[]) {
  const counts: Record<string, number> = {};
  for (const it of items) {
    const k = kindOf(it.ev.payload as Payload);
    const key = k === "execute" ? "ran" : k === "read" ? "read" : k === "edit" || k === "delete" || k === "move" ? "edited" : k === "search" ? "searched" : "used";
    counts[key] = (counts[key] ?? 0) + 1;
  }
  const noun: Record<string, (n: number) => string> = {
    ran: (n) => `ran ${n} command${n > 1 ? "s" : ""}`,
    read: (n) => `read ${n} file${n > 1 ? "s" : ""}`,
    edited: (n) => `edited ${n} file${n > 1 ? "s" : ""}`,
    searched: (n) => `searched ${n} time${n > 1 ? "s" : ""}`,
    used: (n) => `used ${n} tool${n > 1 ? "s" : ""}`,
  };
  const s = Object.entries(counts)
    .map(([k, n]) => noun[k](n))
    .join(", ");
  return s.charAt(0).toUpperCase() + s.slice(1);
}

function ToolGroup({ items, worktree, live }: { items: ToolItem[]; worktree?: string | null; live: boolean }) {
  const running = items.some((i) => ["pending", "in_progress"].includes(String((i.ev.payload as Payload).status ?? "pending")));
  const failed = items.filter((i) => (i.ev.payload as Payload).status === "failed").length;
  const denied = items.filter((i) => typeof i.perm?.decision === "string" && (i.perm.decision as string).startsWith("denied")).length;
  const [open, setOpen] = useState(false);
  const last = items[items.length - 1].ev.payload as Payload;
  return (
    <div className="text-sm">
      <button
        onClick={() => setOpen(!open)}
        className="flex max-w-full items-center gap-2 rounded-lg border border-line bg-surface px-2.5 py-1.5 text-[13px] text-fg-muted shadow-card transition-colors hover:border-line-strong hover:text-fg"
      >
        {running && live ? <Loader2 size={13} className="shrink-0 animate-spin text-sky-500" /> : <Wrench size={13} className="shrink-0 text-fg-subtle" />}
        <span className="shrink-0">{groupLabel(items)}</span>
        {failed > 0 && <span className="shrink-0 text-rose-600 dark:text-rose-400">· {failed} failed</span>}
        {denied > 0 && <span className="shrink-0 text-rose-600 dark:text-rose-400">· {denied} denied</span>}
        {!open && running && live && <span className="min-w-0 truncate text-xs text-fg-subtle">— {summary(last, worktree)}</span>}
        {open ? <ChevronDown size={13} className="shrink-0 text-fg-subtle" /> : <ChevronRight size={13} className="shrink-0 text-fg-subtle" />}
      </button>
      {open && (
        <div className="mt-1.5 ml-3 border-l-2 border-line pl-2">
          {items.map((it) => (
            <ToolRow key={it.ev.seq} item={it} worktree={worktree} />
          ))}
        </div>
      )}
    </div>
  );
}

// ---------- other events ----------

function Collapsible({ title, children, defaultOpen, className }: { title: ReactNode; children: ReactNode; defaultOpen?: boolean; className?: string }) {
  const [open, setOpen] = useState(!!defaultOpen);
  return (
    <div className={className}>
      <button onClick={() => setOpen(!open)} className="flex items-center gap-1.5 text-left text-xs text-fg-muted hover:text-fg">
        {open ? <ChevronDown size={12} className="shrink-0" /> : <ChevronRight size={12} className="shrink-0" />}
        {title}
      </button>
      {open && <div className="mt-2">{children}</div>}
    </div>
  );
}

function PendingPermission({ p }: { p: Payload }) {
  const qc = useQueryClient();
  const answer = useMutation({
    mutationFn: (option_id: string) => api("POST", `/api/permission-requests/${p.id}`, { option_id }),
    onSuccess: () => qc.invalidateQueries(),
  });
  const options = (p.options as { optionId: string; name: string; kind: string }[]) ?? [];
  const tool = (p.tool_call as Payload) ?? {};
  const cmd = commandOf(tool);
  return (
    <div className="space-y-2.5 rounded-xl border border-amber-500/40 bg-amber-500/8 p-3 text-sm">
      <div className="flex items-center gap-2 font-medium text-amber-800 dark:text-amber-300">
        <ShieldAlert size={15} /> Permission needed{p.reason ? <span className="text-xs font-normal text-fg-muted">({String(p.reason)})</span> : null}
      </div>
      {cmd ? (
        <pre className="rounded-lg border border-line bg-surface px-3 py-2 font-mono text-[11px] break-all whitespace-pre-wrap">
          <span className="text-fg-subtle select-none">$ </span>
          <ShellHighlight code={cmd} />
        </pre>
      ) : (
        <div className="text-xs">{String(p.title)}</div>
      )}
      {p.status === "pending" ? (
        <div className="flex flex-wrap gap-2">
          {options.map((o) => (
            <Button key={o.optionId} size="sm" variant={o.kind.startsWith("allow") ? "success" : "default"} onClick={() => answer.mutate(o.optionId)}>
              {o.name}
            </Button>
          ))}
        </div>
      ) : (
        <div className="text-xs text-fg-muted">{String(p.status)}</div>
      )}
    </div>
  );
}

function EventView({ ev }: { ev: RunEvent }) {
  const p = ev.payload as Payload;
  switch (ev.kind) {
    case "prompt":
      return (
        <Collapsible
          className={clsx("rounded-xl border px-3.5 py-2.5", p.from === "human" ? "border-accent/25 bg-accent-soft/50" : "border-line bg-surface-2/60")}
          title={
            <span className="flex items-center gap-1.5 font-medium">
              {p.from === "nudge" ? <Undo2 size={13} /> : p.from === "human" ? <MessageSquare size={13} /> : <ScrollText size={13} />}
              {p.from === "nudge" ? "Nudge" : p.from === "human" ? "Message from you" : "Prompt"}
            </span>
          }
          defaultOpen={p.from === "human" || p.from === "nudge"}
        >
          <Markdown className="text-xs">{String(p.text)}</Markdown>
        </Collapsible>
      );
    case "message":
      return (
        <div className="flex gap-3">
          <Avatar agent size={24} className="mt-0.5" />
          <Markdown className="min-w-0 flex-1">{String(p.text)}</Markdown>
        </div>
      );
    case "thought":
      return (
        <Collapsible title={<span className="flex items-center gap-1.5 italic"><Brain size={13} /> Thinking</span>}>
          <div className="border-l-2 border-line pl-3 text-xs whitespace-pre-wrap text-fg-muted italic">{String(p.text)}</div>
        </Collapsible>
      );
    case "plan": {
      const entries = (p.entries as { content: string; status: string }[]) ?? [];
      return (
        <div className="rounded-xl border border-line bg-surface px-3.5 py-3 text-sm shadow-card">
          <div className="mb-2 text-xs font-semibold text-fg-muted">Plan</div>
          <div className="space-y-1.5">
            {entries.map((e, i) => (
              <div key={i} className={clsx("flex gap-2", e.status === "completed" && "text-fg-subtle line-through")}>
                {e.status === "completed" ? (
                  <CheckCircle2 size={15} className="mt-0.5 shrink-0 text-emerald-500" />
                ) : e.status === "in_progress" ? (
                  <CircleDot size={15} className="mt-0.5 shrink-0 text-sky-500" />
                ) : (
                  <Circle size={15} className="mt-0.5 shrink-0 text-fg-subtle" />
                )}
                <span className="min-w-0">{e.content}</span>
              </div>
            ))}
          </div>
        </div>
      );
    }
    case "stderr":
      return (
        <Collapsible title={<span className="flex items-center gap-1.5"><Terminal size={12} /> Adapter log</span>}>
          <pre className="max-h-48 overflow-auto rounded-lg bg-zinc-950 p-3 font-mono text-[11px] text-zinc-300">{String(p.text)}</pre>
        </Collapsible>
      );
    case "setup":
      return (
        <Collapsible
          title={
            <span className={clsx("flex items-center gap-1.5", !p.ok && "text-rose-600 dark:text-rose-400")}>
              {p.ok ? <CheckCircle2 size={12} className="text-emerald-500" /> : <XCircle size={12} />} Setup script{p.ok ? "" : " failed"}
            </span>
          }
          defaultOpen={!p.ok}
        >
          <pre className="max-h-48 overflow-auto rounded-lg bg-zinc-950 p-3 font-mono text-[11px] text-zinc-300">{String(p.text)}</pre>
        </Collapsible>
      );
    case "permission":
      return <PendingPermission p={p} />;
    case "usage":
      return null;
    default:
      return <div className="text-xs break-words text-fg-subtle">• {String(p.text ?? JSON.stringify(p))}</div>;
  }
}

type Block = { type: "tools"; items: ToolItem[]; key: number } | { type: "event"; ev: RunEvent; key: number };

/** Group consecutive tool calls (and the permission decisions attached to them) into collapsible blocks. */
function blocks(events: RunEvent[]): Block[] {
  const out: Block[] = [];
  const permFor = new Map<string, Payload>();
  for (const e of events) {
    if (e.kind === "permission") {
      const p = e.payload as Payload;
      const id = (p.tool_call as Payload | undefined)?.toolCallId;
      if (typeof id === "string" && p.decision) permFor.set(id, p);
    }
  }
  for (const e of events) {
    const p = e.payload as Payload;
    if (e.kind === "usage") continue;
    // Auto-decided permissions are shown on their tool row; stderr noise folds into the tool group.
    if (e.kind === "permission" && p.decision) continue;
    if (e.kind === "tool_call") {
      const item = { ev: e, perm: permFor.get(String(p.toolCallId)) };
      const last = out[out.length - 1];
      if (last?.type === "tools") last.items.push(item);
      else out.push({ type: "tools", items: [item], key: e.seq });
      continue;
    }
    // Thoughts and adapter logs between tool calls don't break a group.
    if ((e.kind === "thought" || e.kind === "stderr") && out[out.length - 1]?.type === "tools") {
      const next = events.find((x) => x.seq > e.seq && !["thought", "stderr", "usage"].includes(x.kind));
      if (next?.kind === "tool_call") continue;
    }
    // Adjacent pieces of the same message/thought are one block (older transcripts stored
    // streamed text in several rows, sometimes mid-sentence).
    const prev = out[out.length - 1];
    if ((e.kind === "message" || e.kind === "thought") && prev?.type === "event" && prev.ev.kind === e.kind) {
      const text = String((prev.ev.payload as Payload).text ?? "") + String(p.text ?? "");
      prev.ev = { ...prev.ev, payload: { ...(prev.ev.payload as Payload), text } as unknown as RunEvent["payload"] };
      continue;
    }
    out.push({ type: "event", ev: e, key: e.seq });
  }
  return out;
}

export function Transcript({ events, worktree, live }: { events: RunEvent[]; worktree?: string | null; live: boolean }) {
  return (
    <div className="space-y-3.5">
      {blocks(events).map((b) =>
        b.type === "tools" ? <ToolGroup key={b.key} items={b.items} worktree={worktree} live={live} /> : <EventView key={b.key} ev={b.ev} />,
      )}
    </div>
  );
}
