import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import clsx from "clsx";
import { CheckCircle2, Clock, FlaskConical, KeyRound, Pause, Pencil, Play, Plus, XCircle, Zap } from "lucide-react";
import { useState } from "react";
import { type AgentDefinition, type LimitGroup, ROLE_LABEL, type Role, type S, api, client, unwrap } from "../api/client";
import { useConnectClaude, useCredentials } from "../components/ConnectClaude";
import { useSettings } from "../components/Layout";
import { SettingSelects, agentOptions } from "../components/SessionSettings";
import { Button, ErrorBox, Field, Modal, Page, PageHeader, Pill, Section, Switch, TimeAgo, fieldSmCls, inputCls, selectCls } from "../components/ui";

const HARNESS_MARK: Record<string, string> = {
  claude: "from-orange-400 to-amber-600",
  codex: "from-zinc-600 to-zinc-900 dark:from-zinc-300 dark:to-zinc-500",
  opencode: "from-sky-500 to-indigo-600",
};

function HarnessMark({ harness, name }: { harness: string; name: string }) {
  return (
    <span className={clsx("grid h-9 w-9 shrink-0 place-items-center rounded-xl bg-gradient-to-br text-sm font-semibold text-white uppercase shadow-card dark:text-zinc-900", HARNESS_MARK[harness] ?? "from-fuchsia-500 to-violet-600")}>
      {name.charAt(0)}
    </span>
  );
}

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
    <Section title="Dispatch" description="How the scheduler hands work to agents, across every project.">
      <div className="space-y-6">
        <div className="grid gap-4 sm:grid-cols-3">
          <div className="flex items-center justify-between gap-3 rounded-xl border border-line bg-surface-2/50 px-3.5 py-3 sm:col-span-1">
            <div>
              <div className="text-sm font-medium">Scheduler</div>
              <div className="text-xs text-fg-muted">{d.scheduler_enabled ? "Dispatching automatically" : "Only manual runs"}</div>
            </div>
            <Switch checked={d.scheduler_enabled} onChange={(v) => patch.mutate({ scheduler_enabled: v })} label="Scheduler" tone="green" />
          </div>
          <Field label="Max simultaneous agents" hint="Across all projects and agents">
            <input type="number" min={0} max={64} className={inputCls} defaultValue={d.max_concurrent_runs} onBlur={(e) => patch.mutate({ max_concurrent_runs: Number(e.target.value) })} />
          </Field>
          <Field label="Failures before stalling">
            <input type="number" min={1} className={inputCls} defaultValue={d.max_failures} onBlur={(e) => patch.mutate({ max_failures: Number(e.target.value) })} />
          </Field>
        </div>
        <div>
          <div className="mb-2 text-xs font-medium text-fg-muted">Default agent per role</div>
          <div className="grid gap-3 sm:grid-cols-2 lg:grid-cols-4">
            {(["triage", "fix", "review", "merge_prep"] as Role[]).map((r) => (
              <div key={r} className="space-y-2.5 rounded-xl border border-line p-3">
                <div className="text-sm font-semibold">{ROLE_LABEL[r]}</div>
                <select
                  className={clsx(inputCls, selectCls)}
                  value={d.default_role_agents[r] ?? ""}
                  onChange={(e) => patch.mutate({ default_role_agents: { [r]: e.target.value } })}
                  aria-label={`${ROLE_LABEL[r]} agent`}
                >
                  {agents.data?.map((a) => (
                    <option key={a.slug} value={a.slug}>
                      {a.name}
                    </option>
                  ))}
                </select>
                <label className="flex items-center gap-2 text-xs text-fg-muted">
                  Timeout
                  <input
                    type="number"
                    className={clsx(fieldSmCls, "w-20")}
                    defaultValue={d.run_timeouts_minutes[r]}
                    onBlur={(e) => patch.mutate({ run_timeouts_minutes: { [r]: Number(e.target.value) } })}
                    aria-label={`${ROLE_LABEL[r]} timeout in minutes`}
                  />
                  min
                </label>
              </div>
            ))}
          </div>
        </div>
        <ErrorBox error={patch.error} />
      </div>
    </Section>
  );
}

function Snapshot({ s }: { s: Record<string, unknown> }) {
  const util = typeof s.utilization === "number" ? s.utilization : undefined;
  return (
    <span>
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
    <div className="flex flex-wrap items-center gap-x-4 gap-y-2 px-4 py-3.5 text-sm sm:px-5">
      <div className="min-w-0 flex-1 basis-56">
        <div className="flex flex-wrap items-center gap-2">
          <span className="font-medium">{g.name}</span>
          {g.paused ? (
            <Pill tone="amber">
              <Clock size={11} /> Paused ({g.pause_kind})
              {g.paused_until ? (
                <>
                  {" "}
                  · resumes <TimeAgo iso={g.paused_until} />
                </>
              ) : g.next_probe_at ? (
                <>
                  {" "}
                  · probe <TimeAgo iso={g.next_probe_at} />
                </>
              ) : null}
            </Pill>
          ) : (
            <Pill tone="green">Available</Pill>
          )}
        </div>
        <div className="mt-0.5 truncate text-xs text-fg-subtle" title={g.pause_reason ?? ""}>
          {members.join(", ")}
          {members.length > 0 && (g.paused ? g.pause_reason : g.last_snapshot) ? " · " : ""}
          {g.paused ? g.pause_reason : g.last_snapshot ? <Snapshot s={g.last_snapshot as Record<string, unknown>} /> : null}
        </div>
      </div>
      <div className="flex gap-2">
        {g.paused ? (
          <>
            <Button size="sm" onClick={() => act.mutate("probe")} disabled={act.isPending} title="Send a one-line prompt to check if the subscription is back">
              <Zap size={12} /> Probe
            </Button>
            <Button size="sm" variant="primary" onClick={() => act.mutate("resume")}>
              <Play size={12} /> Resume
            </Button>
          </>
        ) : (
          <Button size="sm" onClick={() => act.mutate("pause")}>
            <Pause size={12} /> Pause
          </Button>
        )}
      </div>
      {act.error ? <span className="w-full text-xs text-rose-600 dark:text-rose-400">{(act.error as Error).message}</span> : null}
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
  const policy = (
    <>
      <option value="allowlist">allowlist (rules below, else ask)</option>
      <option value="auto_allow">auto_allow (approve everything)</option>
      <option value="ask">ask (every prompt goes to the Inbox)</option>
      <option value="deny">deny</option>
    </>
  );
  return (
    <Modal open onClose={onClose} title={agent ? `Edit ${agent.name}` : "New agent"} wide>
      <div className="grid gap-4 sm:grid-cols-2">
        <Field label="Slug">
          <input className={inputCls} value={f.slug} onChange={set("slug")} disabled={!!agent} />
        </Field>
        <Field label="Name">
          <input className={inputCls} value={f.name} onChange={set("name")} />
        </Field>
        <Field label="Harness" hint="claude/codex get subscription-limit handling and container credentials">
          <select className={clsx(inputCls, selectCls)} value={f.harness} onChange={set("harness")}>
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
          <textarea className={clsx(inputCls, "h-20 font-mono sm:text-xs")} value={f.args} onChange={set("args")} />
        </Field>
        <Field label="Env (KEY=value per line)">
          <textarea className={clsx(inputCls, "h-20 font-mono sm:text-xs")} value={f.env} onChange={set("env")} />
        </Field>
        <Field label="Permission policy on host" hint="For prompts the agent still raises in its session mode">
          <select className={clsx(inputCls, selectCls)} value={f.permission_policy} onChange={set("permission_policy")}>
            {policy}
          </select>
        </Field>
        <Field label="Permission policy in containers" hint="The container is the sandbox, so auto_allow is typical">
          <select className={clsx(inputCls, selectCls)} value={f.container_permission_policy} onChange={set("container_permission_policy")}>
            {policy}
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
        className="mt-4"
        label="Allowlist rules (JSON, first match wins)"
        hint={
          <>
            Each rule: <code className="font-mono">{`{"action": "allow"|"deny"|"ask", "kinds": ["execute"], "pattern": "regex"}`}</code>. Patterns match the command (every segment of a chained command must match
            an allow rule) or file paths; <code className="font-mono">{"{worktree}"}</code> is the run's worktree. Unmatched prompts go to the Inbox.
          </>
        }
      >
        <textarea className={clsx(inputCls, "h-48 font-mono sm:text-[11px]")} value={f.permission_rules} onChange={set("permission_rules")} />
      </Field>
      <ErrorBox error={save.error} className="mt-4" />
      <div className="mt-5 flex justify-end gap-2 border-t border-line pt-4">
        <Button variant="ghost" onClick={onClose}>
          Cancel
        </Button>
        <Button variant="primary" onClick={() => save.mutate()} disabled={save.isPending}>
          Save agent
        </Button>
      </div>
    </Modal>
  );
}

function TestResult({ r }: { r: S["AgentTestResult"] }) {
  const modes = (r.modes as { availableModes?: { id: string }[] } | null)?.availableModes?.map((m) => m.id);
  return (
    <div className={clsx("space-y-1 rounded-xl border p-3 text-xs", r.ok ? "border-emerald-500/30 bg-emerald-500/6" : "border-rose-500/30 bg-rose-500/6")}>
      <div className="flex items-center gap-1.5 font-medium">
        {r.ok ? <CheckCircle2 size={14} className="text-emerald-500" /> : <XCircle size={14} className="text-rose-500" />}
        {r.ok ? "Handshake OK" : "Test failed"}
        {r.agent_info ? <span className="min-w-0 truncate font-normal text-fg-muted">— {JSON.stringify(r.agent_info)}</span> : ""}
      </div>
      {modes && <div className="text-fg-muted">Modes: {modes.join(", ")}</div>}
      {r.reply && <div className="text-fg-muted">Reply: {r.reply}</div>}
      {r.limit && <div className="text-amber-700 dark:text-amber-300">Limit detected: {r.limit}</div>}
      {r.error && <div className="text-rose-700 dark:text-rose-300">{r.error}</div>}
      {r.stderr_tail && <pre className="max-h-40 overflow-auto rounded-lg bg-zinc-950 p-2 font-mono text-[10px] whitespace-pre-wrap text-zinc-300">{r.stderr_tail}</pre>}
    </div>
  );
}

function AgentSettings({ a, onLoad, loading }: { a: AgentDefinition; onLoad: () => void; loading: boolean }) {
  const qc = useQueryClient();
  const opts = agentOptions(a);
  const patch = useMutation({
    mutationFn: (session_config: Record<string, unknown>) => api("PATCH", `/api/agents/${a.slug}`, { session_config }),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["agents"] }),
  });
  return (
    <div className="flex flex-wrap items-center gap-2">
      {opts.length ? (
        <>
          <SettingSelects options={opts} values={a.session_config as Record<string, unknown>} onChange={(id, v) => patch.mutate({ [id]: v })} inheritLabel={() => "adapter default"} />
          <span className="text-[11px] text-fg-subtle">Defaults for every run; projects can override per role.</span>
        </>
      ) : (
        <Button size="sm" variant="ghost" onClick={onLoad} disabled={loading} className="-ml-2">
          {loading ? "Loading…" : "Load model & effort options"}
        </Button>
      )}
      {patch.error ? <span className="text-xs text-rose-600 dark:text-rose-400">{(patch.error as Error).message}</span> : null}
    </div>
  );
}

function AgentRow({ a }: { a: AgentDefinition }) {
  const qc = useQueryClient();
  const [editing, setEditing] = useState(false);
  const test = useMutation({ mutationFn: (prompt: boolean) => unwrap(client.POST("/api/agents/{slug}/test", { params: { path: { slug: a.slug } }, body: { prompt } })) });
  const toggle = useMutation({ mutationFn: () => api("PATCH", `/api/agents/${a.slug}`, { enabled: !a.enabled }), onSuccess: () => qc.invalidateQueries({ queryKey: ["agents"] }) });
  return (
    <div className={clsx("space-y-3 px-4 py-4 sm:px-5", !a.enabled && "opacity-70")}>
      <div className="flex flex-wrap items-start gap-3">
        <HarnessMark harness={a.harness} name={a.name} />
        <div className="min-w-0 flex-1 basis-48">
          <div className="flex flex-wrap items-center gap-2">
            <span className="font-semibold">{a.name}</span>
            <Pill>{a.harness}</Pill>
            {a.needs_auth && (
              <Pill tone="red">
                <KeyRound size={11} /> Needs login
              </Pill>
            )}
          </div>
          <code className="mt-0.5 block truncate font-mono text-xs text-fg-subtle" title={`${a.command} ${a.args.join(" ")}`}>
            {a.command} {a.args.join(" ")}
          </code>
        </div>
        <div className="flex flex-wrap items-center gap-2">
          <label className="mr-1 flex items-center gap-2 text-xs text-fg-muted">
            <Switch size="sm" checked={a.enabled} onChange={() => toggle.mutate()} label={`${a.name} enabled`} /> Enabled
          </label>
          <Button size="sm" onClick={() => test.mutate(false)} disabled={test.isPending} title="Start the adapter and do the ACP handshake">
            <FlaskConical size={12} /> {test.isPending ? "Testing…" : "Test"}
          </Button>
          <Button size="sm" variant="ghost" onClick={() => test.mutate(true)} disabled={test.isPending} title="Also send a one-line prompt (uses a little quota)">
            + prompt
          </Button>
          <Button size="sm" variant="ghost" icon onClick={() => setEditing(true)} aria-label={`Edit ${a.name}`} title="Edit">
            <Pencil size={13} />
          </Button>
        </div>
      </div>
      <div className="flex flex-wrap gap-1.5 text-[11px] sm:pl-12">
        <span className="rounded-md bg-surface-3/70 px-1.5 py-0.5 text-fg-muted">host: {a.permission_policy}</span>
        <span className="rounded-md bg-surface-3/70 px-1.5 py-0.5 text-fg-muted">container: {a.container_permission_policy}</span>
        <span className="rounded-md bg-surface-3/70 px-1.5 py-0.5 text-fg-muted">max {a.max_concurrent}</span>
        {a.session_mode_id && <span className="rounded-md bg-surface-3/70 px-1.5 py-0.5 text-fg-muted">mode {a.session_mode_id}</span>}
      </div>
      <div className="sm:pl-12">
        <AgentSettings a={a} onLoad={() => test.mutate(false)} loading={test.isPending} />
      </div>
      {test.data && (
        <div className="sm:pl-12">
          <TestResult r={test.data} />
        </div>
      )}
      <ErrorBox error={test.error} />
      {editing && <AgentForm agent={a} onClose={() => setEditing(false)} />}
    </div>
  );
}

function CredentialsCard() {
  const qc = useQueryClient();
  const creds = useCredentials();
  const connect = useConnectClaude();
  const remove = useMutation({ mutationFn: () => api("DELETE", "/api/credentials/claude-token"), onSuccess: () => qc.invalidateQueries() });
  const c = creds.data;
  if (!c) return null;
  const row = (name: string, ok: boolean, detail: string, actions: React.ReactNode) => (
    <div className="flex flex-wrap items-center gap-x-4 gap-y-2 px-4 py-3.5 text-sm sm:px-5">
      <div className="min-w-0 flex-1 basis-60">
        <div className="flex items-center gap-2">
          <span className="font-medium">{name}</span>
          {ok ? <Pill tone="green">Configured</Pill> : <Pill tone={c.container_projects.length ? "red" : "neutral"}>Missing</Pill>}
        </div>
        <div className="mt-0.5 text-xs text-fg-subtle">{detail}</div>
      </div>
      {actions && <div className="flex gap-2">{actions}</div>}
    </div>
  );
  return (
    <Section title="Container credentials" description={c.container_projects.length ? `Used by ${c.container_projects.join(", ")}` : "No project runs agents in containers"} flush>
      <div className="divide-y divide-line">
        {row(
          "Claude",
          c.claude_token,
          "Long-lived token from `claude setup-token`; on the host Claude uses your Keychain login instead.",
          c.claude_token ? (
            <>
              <Button size="sm" onClick={connect}>
                Replace
              </Button>
              <Button size="sm" variant="ghost" onClick={() => confirm("Remove Claude's container token?") && remove.mutate()}>
                Remove
              </Button>
            </>
          ) : (
            <Button size="sm" variant="primary" onClick={connect}>
              Connect Claude
            </Button>
          ),
        )}
        {row("Codex", c.codex_auth, "~/.codex/auth.json is mounted into Codex containers (sign in with the Codex app or CLI to create it).", null)}
      </div>
    </Section>
  );
}

export default function AgentsPage() {
  const agents = useQuery({ queryKey: ["agents"], queryFn: () => unwrap(client.GET("/api/agents")) });
  const groups = useQuery({ queryKey: ["limits"], queryFn: () => unwrap(client.GET("/api/limit-groups")), refetchInterval: 30000 });
  const [creating, setCreating] = useState(false);
  return (
    <Page width="xl">
      <PageHeader
        title="Agents"
        subtitle="Who does the work, how many at once, and how they sign in."
        actions={
          <Button variant="primary" onClick={() => setCreating(true)}>
            <Plus size={15} /> Add agent
          </Button>
        }
      />
      <div className="space-y-6">
        <SettingsCard />
        <Section title="Agents" description="Adapters that speak the Agent Client Protocol." flush>
          <div className="divide-y divide-line">
            {agents.data?.map((a) => (
              <AgentRow key={a.slug} a={a} />
            ))}
          </div>
        </Section>
        <Section title="Subscriptions" description="Agents pause automatically when a usage limit is hit and resume when it resets." flush>
          <div className="divide-y divide-line">
            {groups.data?.map((g) => (
              <LimitGroupRow key={g.name} g={g} agents={agents.data ?? []} />
            ))}
          </div>
        </Section>
        <CredentialsCard />
      </div>
      {creating && <AgentForm onClose={() => setCreating(false)} />}
    </Page>
  );
}
