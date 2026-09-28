import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import clsx from "clsx";
import { Clock, FlaskConical, Pause, Pencil, Play, Plus, Zap } from "lucide-react";
import { useState } from "react";
import { type AgentDefinition, type LimitGroup, ROLE_LABEL, type Role, type S, api, client, unwrap } from "../api/client";
import { useSettings } from "../components/Layout";
import { Button, ErrorBox, Field, Modal, Pill, TimeAgo, inputCls } from "../components/ui";

function SettingsCard() {
  const qc = useQueryClient();
  const s = useSettings();
  const agents = useQuery({ queryKey: ["agents"], queryFn: () => unwrap(client.GET("/api/agents")) });
  const patch = useMutation({
    mutationFn: (body: Record<string, unknown>) => api("PATCH", "/api/settings", body),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["settings"] }),
  });
  if (!s.data) return null;
  const d = s.data;
  return (
    <section className="rounded-lg border border-zinc-200 dark:border-zinc-800 bg-white dark:bg-zinc-900 p-4 space-y-4">
      <h2 className="font-semibold">Dispatch</h2>
      <div className="grid grid-cols-2 md:grid-cols-4 gap-4">
        <Field label="Max simultaneous agents" hint="Across all projects and agents">
          <input type="number" min={0} max={64} className={inputCls} defaultValue={d.max_concurrent_runs} onBlur={(e) => patch.mutate({ max_concurrent_runs: Number(e.target.value) })} />
        </Field>
        <Field label="Scheduler" hint="Dispatch agents automatically">
          <Button variant={d.scheduler_enabled ? "success" : "default"} onClick={() => patch.mutate({ scheduler_enabled: !d.scheduler_enabled })}>
            {d.scheduler_enabled ? <Play size={13} /> : <Pause size={13} />} {d.scheduler_enabled ? "On" : "Off"}
          </Button>
        </Field>
        <Field label="Failures before stalling">
          <input type="number" min={1} className={inputCls} defaultValue={d.max_failures} onBlur={(e) => patch.mutate({ max_failures: Number(e.target.value) })} />
        </Field>
      </div>
      <div className="grid grid-cols-2 md:grid-cols-4 gap-4">
        {(["triage", "fix", "review", "merge_prep"] as Role[]).map((r) => (
          <div key={r} className="space-y-2">
            <Field label={`${ROLE_LABEL[r]} agent (default)`}>
              <select className={inputCls} value={d.default_role_agents[r] ?? ""} onChange={(e) => patch.mutate({ default_role_agents: { [r]: e.target.value } })}>
                {agents.data?.map((a) => (
                  <option key={a.slug} value={a.slug}>
                    {a.name}
                  </option>
                ))}
              </select>
            </Field>
            <Field label="Timeout (min)">
              <input type="number" className={inputCls} defaultValue={d.run_timeouts_minutes[r]} onBlur={(e) => patch.mutate({ run_timeouts_minutes: { [r]: Number(e.target.value) } })} />
            </Field>
          </div>
        ))}
      </div>
      <ErrorBox error={patch.error} />
    </section>
  );
}

function Snapshot({ s }: { s: Record<string, unknown> }) {
  const util = typeof s.utilization === "number" ? s.utilization : undefined;
  return (
    <span className="text-xs text-zinc-500">
      {s.rateLimitType ? `${String(s.rateLimitType).replace("_", " ")} window` : "usage"}
      {util !== undefined && ` · ${Math.round(util * (util <= 1 ? 100 : 1))}% used`}
      {typeof s.resetsAt === "number" && ` · resets ${new Date(s.resetsAt * 1000).toLocaleString()}`}
      {s.status ? ` · ${String(s.status)}` : ""}
    </span>
  );
}

function LimitGroupRow({ g, agents }: { g: LimitGroup; agents: AgentDefinition[] }) {
  const qc = useQueryClient();
  const act = useMutation({
    mutationFn: (a: "pause" | "resume" | "probe") => api("POST", `/api/limit-groups/${g.name}/${a}`, a === "pause" ? {} : undefined),
    onSuccess: () => qc.invalidateQueries(),
  });
  const members = agents.filter((a) => a.limit_group === g.name).map((a) => a.name);
  return (
    <div className="flex items-center gap-3 px-3 py-2 text-sm">
      <span className="w-24 font-medium">{g.name}</span>
      <span className="w-40 text-xs text-zinc-500 truncate">{members.join(", ")}</span>
      {g.paused ? (
        <Pill className="bg-amber-100 text-amber-900 dark:bg-amber-950 dark:text-amber-300">
          <Clock size={11} /> paused ({g.pause_kind}){g.paused_until ? <> · resumes <TimeAgo iso={g.paused_until} /></> : g.next_probe_at ? <> · probe <TimeAgo iso={g.next_probe_at} /></> : null}
        </Pill>
      ) : (
        <Pill className="bg-emerald-100 text-emerald-800 dark:bg-emerald-950 dark:text-emerald-300">available</Pill>
      )}
      <span className="flex-1 truncate text-xs text-zinc-500" title={g.pause_reason ?? ""}>
        {g.paused ? g.pause_reason : g.last_snapshot ? <Snapshot s={g.last_snapshot as Record<string, unknown>} /> : null}
      </span>
      {g.paused ? (
        <>
          <Button size="sm" onClick={() => act.mutate("probe")} disabled={act.isPending} title="Send a one-line prompt to check if the subscription is back">
            <Zap size={11} /> Probe
          </Button>
          <Button size="sm" variant="primary" onClick={() => act.mutate("resume")}>
            <Play size={11} /> Resume
          </Button>
        </>
      ) : (
        <Button size="sm" onClick={() => act.mutate("pause")}>
          <Pause size={11} /> Pause
        </Button>
      )}
      {act.error ? <span className="text-xs text-rose-600">{(act.error as Error).message}</span> : null}
    </div>
  );
}

function AgentForm({ agent, onClose }: { agent?: AgentDefinition; onClose: () => void }) {
  const qc = useQueryClient();
  const [f, setF] = useState({
    slug: agent?.slug ?? "",
    name: agent?.name ?? "",
    harness: agent?.harness ?? "custom",
    command: agent?.command ?? "",
    args: (agent?.args ?? []).join("\n"),
    env: Object.entries(agent?.env ?? {})
      .map(([k, v]) => `${k}=${v}`)
      .join("\n"),
    container_command: (agent?.container_command ?? []).join(" "),
    limit_group: agent?.limit_group ?? "",
    max_concurrent: agent?.max_concurrent ?? 2,
    permission_policy: agent?.permission_policy ?? "allowlist",
    container_permission_policy: agent?.container_permission_policy ?? "auto_allow",
    permission_rules: JSON.stringify(agent?.permission_rules ?? [], null, 2),
    session_mode_id: agent?.session_mode_id ?? "",
    container_session_mode_id: agent?.container_session_mode_id ?? "",
  });
  const save = useMutation({
    mutationFn: () => {
      const body = {
        ...f,
        args: f.args.split("\n").map((s) => s.trim()).filter(Boolean),
        env: Object.fromEntries(
          f.env
            .split("\n")
            .map((l) => l.trim())
            .filter((l) => l.includes("="))
            .map((l) => [l.slice(0, l.indexOf("=")), l.slice(l.indexOf("=") + 1)]),
        ),
        container_command: f.container_command.trim() ? f.container_command.trim().split(/\s+/) : undefined,
        limit_group: f.limit_group || undefined,
        permission_rules: f.permission_rules.trim() && f.permission_rules.trim() !== "[]" ? JSON.parse(f.permission_rules) : agent ? [] : undefined,
      };
      return agent ? api("PATCH", `/api/agents/${agent.slug}`, body) : api("POST", "/api/agents", body);
    },
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["agents"] });
      onClose();
    },
  });
  const set = (k: keyof typeof f) => (e: React.ChangeEvent<HTMLInputElement | HTMLTextAreaElement | HTMLSelectElement>) => setF({ ...f, [k]: e.target.value });
  return (
    <Modal open onClose={onClose} title={agent ? `Edit ${agent.name}` : "New agent"} wide>
      <div className="grid grid-cols-2 gap-3">
        <Field label="Slug">
          <input className={inputCls} value={f.slug} onChange={set("slug")} disabled={!!agent} />
        </Field>
        <Field label="Name">
          <input className={inputCls} value={f.name} onChange={set("name")} />
        </Field>
        <Field label="Harness" hint="claude/codex get subscription-limit handling and container credentials">
          <select className={inputCls} value={f.harness} onChange={set("harness")}>
            {["claude", "codex", "opencode", "custom"].map((h) => (
              <option key={h}>{h}</option>
            ))}
          </select>
        </Field>
        <Field label="Limit group" hint="Agents sharing a subscription share a group">
          <input className={inputCls} value={f.limit_group} onChange={set("limit_group")} placeholder={f.harness} />
        </Field>
        <Field label="Command (speaks ACP on stdio)">
          <input className={clsx(inputCls, "font-mono")} value={f.command} onChange={set("command")} />
        </Field>
        <Field label="Container command" hint="argv inside the project image">
          <input className={clsx(inputCls, "font-mono")} value={f.container_command} onChange={set("container_command")} />
        </Field>
        <Field label="Args (one per line)">
          <textarea className={clsx(inputCls, "font-mono h-20 text-xs")} value={f.args} onChange={set("args")} />
        </Field>
        <Field label="Env (KEY=value per line)">
          <textarea className={clsx(inputCls, "font-mono h-20 text-xs")} value={f.env} onChange={set("env")} />
        </Field>
        <Field label="Permission policy on host" hint="For prompts the agent still raises in its session mode">
          <select className={inputCls} value={f.permission_policy} onChange={set("permission_policy")}>
            <option value="allowlist">allowlist (rules below, else ask)</option>
            <option value="auto_allow">auto_allow (approve everything)</option>
            <option value="ask">ask (every prompt goes to the Inbox)</option>
            <option value="deny">deny</option>
          </select>
        </Field>
        <Field label="Permission policy in containers" hint="The container is the sandbox, so auto_allow is typical">
          <select className={inputCls} value={f.container_permission_policy} onChange={set("container_permission_policy")}>
            <option value="allowlist">allowlist (rules below, else ask)</option>
            <option value="auto_allow">auto_allow (approve everything)</option>
            <option value="ask">ask (every prompt goes to the Inbox)</option>
            <option value="deny">deny</option>
          </select>
        </Field>
        <Field label="Max concurrent runs of this agent">
          <input type="number" className={inputCls} value={f.max_concurrent} onChange={(e) => setF({ ...f, max_concurrent: Number(e.target.value) })} />
        </Field>
        <Field label="Session mode (host)" hint="Claude: auto, acceptEdits, default · Codex: agent, read-only">
          <input className={inputCls} value={f.session_mode_id} onChange={set("session_mode_id")} />
        </Field>
        <Field label="Session mode (container)" hint="Claude: bypassPermissions · Codex: agent-full-access">
          <input className={inputCls} value={f.container_session_mode_id} onChange={set("container_session_mode_id")} />
        </Field>
      </div>
      <Field
        label="Allowlist rules (JSON, first match wins)"
        hint={<>Each rule: <code>{`{"action": "allow"|"deny"|"ask", "kinds": ["execute"], "pattern": "regex"}`}</code>. Patterns match the command (every segment of a chained command must match an allow rule) or file paths; <code>{"{worktree}"}</code> is the run's worktree. Unmatched prompts go to the Inbox.</>}
      >
        <textarea className={clsx(inputCls, "font-mono h-48 text-[11px]")} value={f.permission_rules} onChange={set("permission_rules")} />
      </Field>
      <ErrorBox error={save.error} />
      <div className="mt-3 flex justify-end">
        <Button variant="primary" onClick={() => save.mutate()} disabled={save.isPending}>
          Save
        </Button>
      </div>
    </Modal>
  );
}

function TestResult({ r }: { r: S["AgentTestResult"] }) {
  const modes = (r.modes as { availableModes?: { id: string }[] } | null)?.availableModes?.map((m) => m.id);
  return (
    <div className={clsx("mt-2 rounded-md border p-2 text-xs space-y-1", r.ok ? "border-emerald-300 bg-emerald-50 dark:bg-emerald-950/30" : "border-rose-300 bg-rose-50 dark:bg-rose-950/30")}>
      <div>
        {r.ok ? "✅ OK" : "❌ Failed"} {r.agent_info ? `— ${JSON.stringify(r.agent_info)}` : ""}
      </div>
      {modes && <div>modes: {modes.join(", ")}</div>}
      {r.reply && <div>reply: {r.reply}</div>}
      {r.limit && <div>limit detected: {r.limit}</div>}
      {r.error && <div className="text-rose-700">{r.error}</div>}
      {r.stderr_tail && (
        <pre className="max-h-40 overflow-auto whitespace-pre-wrap text-[10px] text-zinc-500">{r.stderr_tail}</pre>
      )}
    </div>
  );
}

function AgentRow({ a }: { a: AgentDefinition }) {
  const qc = useQueryClient();
  const [editing, setEditing] = useState(false);
  const test = useMutation({ mutationFn: (prompt: boolean) => unwrap(client.POST("/api/agents/{slug}/test", { params: { path: { slug: a.slug } }, body: { prompt } })) });
  const toggle = useMutation({ mutationFn: () => api("PATCH", `/api/agents/${a.slug}`, { enabled: !a.enabled }), onSuccess: () => qc.invalidateQueries({ queryKey: ["agents"] }) });
  return (
    <div className="px-3 py-2 text-sm">
      <div className="flex items-center gap-3">
        <span className="w-28 font-medium">{a.name}</span>
        <Pill className="bg-zinc-100 dark:bg-zinc-800">{a.harness}</Pill>
        <code className="flex-1 truncate text-xs text-zinc-500">
          {a.command} {a.args.join(" ")}
        </code>
        <span className="text-xs text-zinc-500">
          host: {a.permission_policy} · container: {a.container_permission_policy} · max {a.max_concurrent}
          {a.session_mode_id ? ` · ${a.session_mode_id}` : ""}
        </span>
        {a.needs_auth && <Pill className="bg-rose-100 text-rose-800">needs login</Pill>}
        <label className="flex items-center gap-1 text-xs">
          <input type="checkbox" checked={a.enabled} onChange={() => toggle.mutate()} /> enabled
        </label>
        <Button size="sm" onClick={() => test.mutate(false)} disabled={test.isPending} title="Start the adapter and do the ACP handshake">
          <FlaskConical size={11} /> {test.isPending ? "Testing…" : "Test"}
        </Button>
        <Button size="sm" variant="ghost" onClick={() => test.mutate(true)} disabled={test.isPending} title="Also send a one-line prompt (uses a little quota)">
          + prompt
        </Button>
        <Button size="sm" variant="ghost" onClick={() => setEditing(true)}>
          <Pencil size={11} />
        </Button>
      </div>
      {test.data && <TestResult r={test.data} />}
      <ErrorBox error={test.error} />
      {editing && <AgentForm agent={a} onClose={() => setEditing(false)} />}
    </div>
  );
}

export default function AgentsPage() {
  const agents = useQuery({ queryKey: ["agents"], queryFn: () => unwrap(client.GET("/api/agents")) });
  const groups = useQuery({ queryKey: ["limits"], queryFn: () => unwrap(client.GET("/api/limit-groups")), refetchInterval: 30000 });
  const [creating, setCreating] = useState(false);
  return (
    <div className="mx-auto max-w-6xl p-6 space-y-6">
      <SettingsCard />
      <section className="rounded-lg border border-zinc-200 dark:border-zinc-800 bg-white dark:bg-zinc-900">
        <div className="flex items-center border-b border-zinc-200 dark:border-zinc-800 px-3 py-2">
          <h2 className="font-semibold">Subscriptions</h2>
          <span className="ml-2 text-xs text-zinc-500">Agents pause automatically when a usage limit is hit and resume when it resets.</span>
        </div>
        <div className="divide-y divide-zinc-100 dark:divide-zinc-800">
          {groups.data?.map((g) => (
            <LimitGroupRow key={g.name} g={g} agents={agents.data ?? []} />
          ))}
        </div>
      </section>
      <section className="rounded-lg border border-zinc-200 dark:border-zinc-800 bg-white dark:bg-zinc-900">
        <div className="flex items-center border-b border-zinc-200 dark:border-zinc-800 px-3 py-2">
          <h2 className="font-semibold">Agents (ACP)</h2>
          <Button size="sm" className="ml-auto" onClick={() => setCreating(true)}>
            <Plus size={12} /> Add agent
          </Button>
        </div>
        <div className="divide-y divide-zinc-100 dark:divide-zinc-800">
          {agents.data?.map((a) => (
            <AgentRow key={a.slug} a={a} />
          ))}
        </div>
      </section>
      {creating && <AgentForm onClose={() => setCreating(false)} />}
    </div>
  );
}
