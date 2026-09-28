import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import clsx from "clsx";
import { Hammer, RefreshCw } from "lucide-react";
import { useEffect, useState } from "react";
import { useNavigate, useParams } from "react-router";
import { type Project, ROLE_LABEL, type Role, api, client, unwrap } from "../api/client";
import { SettingSelects, agentOptions, settingLabel, valueName } from "../components/SessionSettings";
import { useSettings } from "../components/Layout";
import { Button, ErrorBox, Field, Pill, TimeAgo, inputCls } from "../components/ui";

function Section({ title, children, desc }: { title: string; desc?: string; children: React.ReactNode }) {
  return (
    <section className="rounded-lg border border-zinc-200 dark:border-zinc-800 bg-white dark:bg-zinc-900 p-4 space-y-3">
      <div>
        <h2 className="font-semibold">{title}</h2>
        {desc && <p className="text-xs text-zinc-500">{desc}</p>}
      </div>
      {children}
    </section>
  );
}

function JobLog({ kind, projectId }: { kind: string; projectId: number }) {
  const jobs = useQuery({
    queryKey: ["jobs", kind],
    queryFn: () => unwrap(client.GET("/api/jobs", { params: { query: { kind, limit: 1 } } })),
    refetchInterval: (q) => (q.state.data?.[0]?.status === "running" || q.state.data?.[0]?.status === "queued" ? 1500 : false),
  });
  const j = jobs.data?.find((x) => x.project_id === projectId);
  if (!j) return null;
  return (
    <div className="space-y-1">
      <div className="text-xs text-zinc-500">
        Last {kind}: <Pill className={j.status === "succeeded" ? "bg-emerald-100 text-emerald-800" : j.status === "failed" ? "bg-rose-100 text-rose-800" : "bg-sky-100 text-sky-800"}>{j.status}</Pill>{" "}
        <TimeAgo iso={j.created_at} />
      </div>
      {j.log && <pre className="max-h-48 overflow-auto rounded bg-zinc-900 text-zinc-300 p-2 text-[11px]">{j.log.split("\n").slice(-60).join("\n")}</pre>}
      {j.error && <div className="text-xs text-rose-600">{j.error}</div>}
    </div>
  );
}

function Roles({ slug }: { slug: string }) {
  const qc = useQueryClient();
  const roles = useQuery({ queryKey: ["project", "roles", slug], queryFn: () => unwrap(client.GET("/api/projects/{p}/roles", { params: { path: { p: slug } } })) });
  const agents = useQuery({ queryKey: ["agents"], queryFn: () => unwrap(client.GET("/api/agents")) });
  const put = useMutation({
    mutationFn: (body: { roles: Record<string, string>; config?: Record<string, Record<string, unknown>> }) => api("PUT", `/api/projects/${slug}/roles`, body),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["project"] }),
  });
  const current = roles.data?.roles ?? {};
  return (
    <div className="space-y-2">
      {(["triage", "fix", "review", "merge_prep"] as Role[]).map((r) => {
        const agent = agents.data?.find((a) => a.slug === current[r]);
        const overrides = (roles.data?.config?.[r] ?? {}) as Record<string, unknown>;
        const defaults = (agent?.session_config ?? {}) as Record<string, unknown>;
        const opts = agentOptions(agent, ["model", "thought_level"]);
        return (
          <div key={r} className="flex flex-wrap items-center gap-3">
            <span className="w-24 text-sm font-medium">{ROLE_LABEL[r]}</span>
            <select className={clsx(inputCls, "w-40 py-1")} value={current[r] ?? ""} onChange={(e) => put.mutate({ roles: { [r]: e.target.value } })}>
              {agents.data?.map((a) => (
                <option key={a.slug} value={a.slug}>
                  {a.name}
                </option>
              ))}
            </select>
            {opts.length ? (
              <SettingSelects
                options={opts}
                values={overrides}
                onChange={(id, v) => put.mutate({ roles: current, config: { [r]: { [id]: v } } })}
                inheritLabel={(o) => (defaults[o.id] != null ? `${settingLabel(o)}: agent default (${valueName(o, defaults[o.id])})` : `${settingLabel(o)}: agent default`)}
              />
            ) : (
              <span className="text-xs text-zinc-500">Model/effort options appear once this agent has run or been tested (Agents page).</span>
            )}
          </div>
        );
      })}
      <ErrorBox error={put.error} />
    </div>
  );
}

function Prompts({ slug }: { slug: string }) {
  const qc = useQueryClient();
  const [role, setRole] = useState<Role>("fix");
  const t = useQuery({ queryKey: ["project", "prompt", slug, role], queryFn: () => unwrap(client.GET("/api/projects/{p}/prompts/{role}", { params: { path: { p: slug, role } } })) });
  const [body, setBody] = useState("");
  useEffect(() => setBody(t.data?.body ?? ""), [t.data]);
  const save = useMutation({
    mutationFn: (b: string) => api("PUT", `/api/projects/${slug}/prompts/${role}`, { role, body: b }),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["project", "prompt"] }),
  });
  return (
    <div className="space-y-2">
      <div className="flex items-center gap-2">
        {(["triage", "fix", "review", "merge_prep"] as Role[]).map((r) => (
          <Button key={r} size="sm" variant={r === role ? "primary" : "default"} onClick={() => setRole(r)}>
            {ROLE_LABEL[r]}
          </Button>
        ))}
        {t.data?.customized && <Pill className="bg-violet-100 text-violet-800">customized</Pill>}
      </div>
      <textarea className={clsx(inputCls, "font-mono text-xs h-96")} value={body} onChange={(e) => setBody(e.target.value)} />
      <div className="flex gap-2">
        <Button variant="primary" onClick={() => save.mutate(body)}>
          Save template
        </Button>
        {t.data?.customized && <Button onClick={() => save.mutate("")}>Reset to default</Button>}
        <span className="text-xs text-zinc-500 self-center">minijinja; variables: project, issue, comments, decisions, pr, threads, latest_review, worktree, branch, api, token</span>
      </div>
      <ErrorBox error={save.error} />
    </div>
  );
}

export default function ProjectSettings() {
  const { slug = "" } = useParams();
  const qc = useQueryClient();
  const p = useQuery({ queryKey: ["project", slug], queryFn: () => unwrap(client.GET("/api/projects/{p}", { params: { path: { p: slug } } })) });
  const [f, setF] = useState<Partial<Project>>({});
  useEffect(() => {
    if (p.data) setF(p.data);
  }, [p.data]);
  const save = useMutation({
    mutationFn: (body: Record<string, unknown>) => api("PATCH", `/api/projects/${slug}`, body),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["project"] }),
  });
  const importGh = useMutation({ mutationFn: () => api("POST", `/api/projects/${slug}/github/import`), onSuccess: () => qc.invalidateQueries({ queryKey: ["jobs"] }) });
  const settings = useSettings();
  const build = useMutation({ mutationFn: () => api("POST", `/api/projects/${slug}/container/build`), onSuccess: () => qc.invalidateQueries({ queryKey: ["jobs"] }) });
  if (!p.data) return <ErrorBox error={p.error} />;
  const str = (k: keyof Project) => ({
    value: (f[k] as string | null | undefined) ?? "",
    onChange: (e: React.ChangeEvent<HTMLInputElement | HTMLTextAreaElement | HTMLSelectElement>) => setF({ ...f, [k]: e.target.value }),
    onBlur: () => f[k] !== p.data![k] && save.mutate({ [k]: f[k] ?? "" }),
  });
  const bool = (k: keyof Project) => ({
    checked: !!f[k],
    onChange: (e: React.ChangeEvent<HTMLInputElement>) => {
      setF({ ...f, [k]: e.target.checked });
      save.mutate({ [k]: e.target.checked });
    },
  });
  return (
    <div className="mx-auto max-w-5xl p-6 space-y-5">
      <h1 className="text-xl font-semibold">{p.data.name} settings</h1>
      <ErrorBox error={save.error} />
      <Section title="Repository">
        <div className="grid grid-cols-2 gap-3">
          <Field label="Name">
            <input className={inputCls} {...str("name")} />
          </Field>
          <Field label="Repository path">
            <input className={clsx(inputCls, "font-mono")} {...str("repo_path")} />
          </Field>
          <Field label="Base branch">
            <input className={inputCls} {...str("base_branch")} />
          </Field>
          <Field label="Branch prefix for agent branches">
            <input className={inputCls} {...str("branch_prefix")} />
          </Field>
          <Field label="Merge strategy">
            <select className={inputCls} {...str("merge_strategy")}>
              <option value="squash">squash</option>
              <option value="merge">merge commit</option>
              <option value="rebase">rebase</option>
            </select>
          </Field>
          <Field label="Commit message regex" hint="Enforced on merge commits and PR titles. Emojicode: ^\p{Extended_Pictographic}">
            <input className={clsx(inputCls, "font-mono")} {...str("commit_msg_regex")} />
          </Field>
          <Field label="Max concurrent runs in this project" hint="Blank = only the global limit">
            <input
              type="number"
              className={inputCls}
              defaultValue={p.data.max_concurrent_runs ?? ""}
              onBlur={(e) => save.mutate({ max_concurrent_runs: Number(e.target.value || 0) })}
            />
          </Field>
        </div>
        <Field label="Worktree setup script" hint="Runs once (bash) in each new agent worktree, e.g. configure the build directory">
          <textarea className={clsx(inputCls, "font-mono text-xs h-24")} {...str("setup_script")} />
        </Field>
      </Section>
      <Section title="Agents" desc="Which agent handles each step of the workflow in this project, and its model and effort for that step.">
        <Roles slug={slug} />
        <Field label="Project instructions for agents" hint="Injected into every prompt (build/test commands, conventions). Replaces board-related parts of AGENTS.md.">
          <textarea className={clsx(inputCls, "font-mono text-xs h-40")} {...str("agent_instructions")} />
        </Field>
      </Section>
      <Section title="Prompt templates">
        <Prompts slug={slug} />
      </Section>
      <Section title="Container sandbox" desc="Run agents in a per-project Docker image. The worktree and the repo's .git are mounted; no GitHub credentials enter the container.">
        <label className="flex items-center gap-2 text-sm">
          <input type="checkbox" {...bool("container_enabled")} /> Run agents in containers
        </label>
        {settings.data && !settings.data.host_agents_allowed && !p.data.container_enabled && (
          <p className="rounded-md border border-amber-300 bg-amber-50 p-2 text-xs text-amber-900 dark:border-amber-900 dark:bg-amber-950/30 dark:text-amber-300">
            This project's agents won't run until containers are on: the server only runs agents in Docker. (Starting it with{" "}
            <code>--dangerously-allow-host-agents</code> lets agents run unsandboxed on this machine.)
          </p>
        )}
        <Field label="Base Dockerfile (toolchain)" hint="An overlay adding Node, git, the ACP adapters and a non-root user is built on top.">
          <textarea className={clsx(inputCls, "font-mono text-xs h-40")} {...str("container_dockerfile")} />
        </Field>
        <div className="flex items-center gap-2">
          <Button onClick={() => build.mutate()} disabled={build.isPending}>
            <Hammer size={13} /> Build image
          </Button>
          {p.data.container_image && <code className="text-xs text-zinc-500">{p.data.container_image}</code>}
        </div>
        <JobLog kind="container.build" projectId={p.data.id} />
      </Section>
      <Section title="GitHub" desc="Import issues, PRs and board status; optionally mirror local activity back.">
        <div className="grid grid-cols-3 gap-3">
          <Field label="Repository (owner/name)">
            <input className={inputCls} {...str("github_repo")} />
          </Field>
          <Field label="Project board owner">
            <input className={inputCls} {...str("github_project_owner")} />
          </Field>
          <Field label="Project board number">
            <input type="number" className={inputCls} defaultValue={p.data.github_project_number ?? ""} onBlur={(e) => save.mutate({ github_project_number: Number(e.target.value) })} />
          </Field>
        </div>
        <div className="flex flex-wrap gap-4 text-sm">
          <label className="flex items-center gap-1">
            <input type="checkbox" {...bool("mirror_push_branches")} /> Push branches & merges
          </label>
          <label className="flex items-center gap-1">
            <input type="checkbox" {...bool("mirror_create_prs")} /> Open GitHub PRs
          </label>
          <label className="flex items-center gap-1">
            <input type="checkbox" {...bool("mirror_sync_status")} /> Sync board status
          </label>
          <label className="flex items-center gap-1">
            <input type="checkbox" {...bool("mirror_create_issues")} /> Create GitHub issues for local ones
          </label>
          <label className="flex items-center gap-1">
            <input type="checkbox" {...bool("mirror_post_verdicts")} /> Post review verdicts
          </label>
        </div>
        <Button onClick={() => importGh.mutate()} disabled={importGh.isPending}>
          <RefreshCw size={13} /> Import / re-sync from GitHub
        </Button>
        <ErrorBox error={importGh.error} />
        <JobLog kind="github.import" projectId={p.data.id} />
      </Section>
    </div>
  );
}

export function NewProjectPage() {
  const nav = useNavigate();
  const qc = useQueryClient();
  const [f, setF] = useState({ slug: "", repo_path: "", base_branch: "", github_repo: "", github_project_owner: "", github_project_number: "" });
  const create = useMutation({
    mutationFn: () =>
      api("POST", "/api/projects", {
        slug: f.slug,
        repo_path: f.repo_path,
        base_branch: f.base_branch || null,
        github_repo: f.github_repo || null,
        github_project_owner: f.github_project_owner || null,
        github_project_number: f.github_project_number ? Number(f.github_project_number) : null,
      }),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["projects"] });
      nav(`/p/${f.slug}/settings`);
    },
  });
  const set = (k: keyof typeof f) => (e: React.ChangeEvent<HTMLInputElement>) => setF({ ...f, [k]: e.target.value });
  return (
    <div className="mx-auto max-w-xl p-6 space-y-4">
      <h1 className="text-xl font-semibold">Add a project</h1>
      <Field label="Slug">
        <input className={inputCls} value={f.slug} onChange={set("slug")} placeholder="emojicode" />
      </Field>
      <Field label="Path to git checkout">
        <input className={clsx(inputCls, "font-mono")} value={f.repo_path} onChange={set("repo_path")} placeholder="/Users/you/workspace/emojicode" />
      </Field>
      <Field label="Base branch (optional)">
        <input className={inputCls} value={f.base_branch} onChange={set("base_branch")} placeholder="master" />
      </Field>
      <Field label="GitHub repo (optional)">
        <input className={inputCls} value={f.github_repo} onChange={set("github_repo")} placeholder="DylanJones/emojicode" />
      </Field>
      <div className="grid grid-cols-2 gap-3">
        <Field label="Board owner">
          <input className={inputCls} value={f.github_project_owner} onChange={set("github_project_owner")} placeholder="DylanJones" />
        </Field>
        <Field label="Board number">
          <input className={inputCls} value={f.github_project_number} onChange={set("github_project_number")} placeholder="1" />
        </Field>
      </div>
      <ErrorBox error={create.error} />
      <Button variant="primary" onClick={() => create.mutate()} disabled={!f.slug || !f.repo_path}>
        Create project
      </Button>
    </div>
  );
}
