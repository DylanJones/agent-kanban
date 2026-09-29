import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import clsx from "clsx";
import { FolderGit2, Hammer, RefreshCw } from "lucide-react";
import { type ReactNode, useEffect, useState } from "react";
import { useNavigate, useParams } from "react-router";
import { type Project, ROLE_LABEL, type Role, api, client, unwrap } from "../api/client";
import { useSettings } from "../components/Layout";
import { SettingSelects, agentOptions, settingLabel, valueName } from "../components/SessionSettings";
import { Button, ErrorBox, Field, Page, PageHeader, Pill, ProjectMark, Section, Segmented, Switch, TimeAgo, fieldCls, inputCls, selectCls } from "../components/ui";

function JobLog({ kind, projectId }: { kind: string; projectId: number }) {
  const jobs = useQuery({
    queryKey: ["jobs", kind],
    queryFn: () => unwrap(client.GET("/api/jobs", { params: { query: { kind, limit: 1 } } })),
    refetchInterval: (q) => (q.state.data?.[0]?.status === "running" || q.state.data?.[0]?.status === "queued" ? 1500 : false),
  });
  const j = jobs.data?.find((x) => x.project_id === projectId);
  if (!j) return null;
  return (
    <div className="space-y-2">
      <div className="flex items-center gap-2 text-xs text-fg-muted">
        Last {kind}: <Pill tone={j.status === "succeeded" ? "green" : j.status === "failed" ? "red" : "sky"}>{j.status}</Pill>
        <TimeAgo iso={j.created_at} className="text-fg-subtle" />
      </div>
      {j.log && <pre className="max-h-56 overflow-auto rounded-xl bg-zinc-950 p-3 font-mono text-[11px] leading-relaxed text-zinc-300">{j.log.split("\n").slice(-60).join("\n")}</pre>}
      {j.error && <div className="text-xs text-rose-600 dark:text-rose-400">{j.error}</div>}
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
    <div className="divide-y divide-line overflow-hidden rounded-xl border border-line">
      {(["triage", "fix", "review", "merge_prep"] as Role[]).map((r) => {
        const agent = agents.data?.find((a) => a.slug === current[r]);
        const overrides = (roles.data?.config?.[r] ?? {}) as Record<string, unknown>;
        const defaults = (agent?.session_config ?? {}) as Record<string, unknown>;
        const opts = agentOptions(agent, ["model", "thought_level"]);
        return (
          <div key={r} className="flex flex-wrap items-center gap-x-4 gap-y-2 px-3.5 py-3">
            <span className="w-24 text-sm font-semibold">{ROLE_LABEL[r]}</span>
            <select className={clsx(fieldCls, selectCls, "w-44")} value={current[r] ?? ""} onChange={(e) => put.mutate({ roles: { [r]: e.target.value } })} aria-label={`${ROLE_LABEL[r]} agent`}>
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
              <span className="text-xs text-fg-subtle">Model/effort options appear once this agent has run or been tested (Agents page).</span>
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
    <div className="space-y-3">
      <div className="flex flex-wrap items-center gap-2">
        <Segmented label="Role" value={role} onChange={setRole} options={(["triage", "fix", "review", "merge_prep"] as Role[]).map((r) => ({ value: r, label: ROLE_LABEL[r] }))} />
        {t.data?.customized && <Pill tone="violet">Customized</Pill>}
      </div>
      <textarea className={clsx(inputCls, "h-96 resize-y font-mono sm:text-xs")} value={body} onChange={(e) => setBody(e.target.value)} aria-label={`${ROLE_LABEL[role]} prompt template`} />
      <div className="flex flex-wrap items-center gap-2">
        <Button variant="primary" onClick={() => save.mutate(body)}>
          Save template
        </Button>
        {t.data?.customized && <Button onClick={() => save.mutate("")}>Reset to default</Button>}
        <span className="text-xs text-fg-subtle">minijinja; variables: project, issue, comments, decisions, pr, threads, latest_review, worktree, branch, api, token</span>
      </div>
      <ErrorBox error={save.error} />
    </div>
  );
}

function Toggle({ label, hint, ...props }: { label: string; hint?: string; checked: boolean; onChange: (v: boolean) => void }) {
  return (
    <label className="flex cursor-pointer items-center justify-between gap-4 py-2.5">
      <div className="min-w-0">
        <div className="text-sm">{label}</div>
        {hint && <div className="text-xs text-fg-subtle">{hint}</div>}
      </div>
      <Switch label={label} {...props} />
    </label>
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
  if (!p.data)
    return (
      <Page>
        <ErrorBox error={p.error} />
      </Page>
    );
  const str = (k: keyof Project) => ({
    value: (f[k] as string | null | undefined) ?? "",
    onChange: (e: React.ChangeEvent<HTMLInputElement | HTMLTextAreaElement | HTMLSelectElement>) => setF({ ...f, [k]: e.target.value }),
    onBlur: () => f[k] !== p.data![k] && save.mutate({ [k]: f[k] ?? "" }),
  });
  const bool = (k: keyof Project) => ({
    checked: !!f[k],
    onChange: (v: boolean) => {
      setF({ ...f, [k]: v });
      save.mutate({ [k]: v });
    },
  });
  return (
    <Page>
      <PageHeader
        title={
          <span className="flex items-center gap-3">
            <ProjectMark name={p.data.name} size={32} />
            {p.data.name}
          </span>
        }
        subtitle="Repository, agents, prompts, sandbox and GitHub sync for this project. Changes save as you go."
      />
      <div className="space-y-6">
        <ErrorBox error={save.error} />
        <Section title="Repository">
          <div className="grid gap-4 sm:grid-cols-2">
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
              <select className={clsx(inputCls, selectCls)} {...str("merge_strategy")}>
                <option value="squash">squash</option>
                <option value="merge">merge commit</option>
                <option value="rebase">rebase</option>
              </select>
            </Field>
            <Field label="Commit message regex" hint="Enforced on merge commits and PR titles. Emojicode: ^\p{Extended_Pictographic}">
              <input className={clsx(inputCls, "font-mono")} {...str("commit_msg_regex")} />
            </Field>
            <Field label="Max concurrent runs in this project" hint="Blank = only the global limit">
              <input type="number" className={inputCls} defaultValue={p.data.max_concurrent_runs ?? ""} onBlur={(e) => save.mutate({ max_concurrent_runs: Number(e.target.value || 0) })} />
            </Field>
          </div>
          <Field className="mt-4" label="Worktree setup script" hint="Runs once (bash) in each new agent worktree, e.g. configure the build directory">
            <textarea className={clsx(inputCls, "h-24 resize-y font-mono sm:text-xs")} {...str("setup_script")} />
          </Field>
        </Section>
        <Section title="Agents" description="Which agent handles each step of the workflow in this project, and its model and effort for that step.">
          <div className="space-y-4">
            <Roles slug={slug} />
            <Field label="Project instructions for agents" hint="Injected into every prompt (build/test commands, conventions). Replaces board-related parts of AGENTS.md.">
              <textarea className={clsx(inputCls, "h-40 resize-y font-mono sm:text-xs")} {...str("agent_instructions")} />
            </Field>
          </div>
        </Section>
        <Section title="Prompt templates">
          <Prompts slug={slug} />
        </Section>
        <Section title="Container sandbox" description="Run agents in a per-project Docker image. The worktree and the repo's .git are mounted; no GitHub credentials enter the container.">
          <div className="space-y-4">
            <Toggle label="Run agents in containers" {...bool("container_enabled")} />
            {settings.data && !settings.data.host_agents_allowed && !p.data.container_enabled && (
              <p className="rounded-xl border border-amber-500/30 bg-amber-500/8 p-3 text-xs text-amber-900 dark:text-amber-300">
                This project's agents won't run until containers are on: the server only runs agents in Docker. (Starting it with <code className="font-mono">--dangerously-allow-host-agents</code> lets
                agents run unsandboxed on this machine.)
              </p>
            )}
            <Field label="Base Dockerfile (toolchain)" hint="An overlay adding Node, git, the ACP adapters and a non-root user is built on top.">
              <textarea className={clsx(inputCls, "h-40 resize-y font-mono sm:text-xs")} {...str("container_dockerfile")} />
            </Field>
            <div className="flex flex-wrap items-center gap-3">
              <Button onClick={() => build.mutate()} disabled={build.isPending}>
                <Hammer size={14} /> Build image
              </Button>
              {p.data.container_image && <code className="font-mono text-xs text-fg-subtle">{p.data.container_image}</code>}
            </div>
            <JobLog kind="container.build" projectId={p.data.id} />
          </div>
        </Section>
        <Section title="GitHub" description="Import issues, PRs and board status; optionally mirror local activity back.">
          <div className="space-y-4">
            <div className="grid gap-4 sm:grid-cols-3">
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
            <div className="divide-y divide-line rounded-xl border border-line px-3.5">
              <Toggle label="Push branches & merges" {...bool("mirror_push_branches")} />
              <Toggle label="Open GitHub PRs" {...bool("mirror_create_prs")} />
              <Toggle label="Sync board status" {...bool("mirror_sync_status")} />
              <Toggle label="Create GitHub issues for local ones" {...bool("mirror_create_issues")} />
              <Toggle label="Post review verdicts" {...bool("mirror_post_verdicts")} />
            </div>
            <Button onClick={() => importGh.mutate()} disabled={importGh.isPending}>
              <RefreshCw size={14} className={clsx(importGh.isPending && "animate-spin")} /> Import / re-sync from GitHub
            </Button>
            <ErrorBox error={importGh.error} />
            <JobLog kind="github.import" projectId={p.data.id} />
          </div>
        </Section>
      </div>
    </Page>
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
  const group = (title: string, children: ReactNode) => (
    <div className="space-y-4">
      <div className="text-xs font-semibold text-fg-muted">{title}</div>
      {children}
    </div>
  );
  return (
    <Page width="sm">
      <div className="mb-6 flex flex-col items-center text-center">
        <div className="mb-3 grid h-12 w-12 place-items-center rounded-2xl bg-accent-soft text-accent-fg">
          <FolderGit2 size={24} />
        </div>
        <h1 className="text-2xl font-semibold tracking-tight">Add a project</h1>
        <p className="mt-1 text-sm text-fg-muted">Point agent-kanban at a git checkout. GitHub is optional.</p>
      </div>
      <form
        className="space-y-6 rounded-2xl border border-line bg-surface p-5 shadow-card sm:p-6"
        onSubmit={(e) => {
          e.preventDefault();
          create.mutate();
        }}
      >
        {group(
          "Repository",
          <>
            <Field label="Slug">
              <input className={inputCls} value={f.slug} onChange={set("slug")} placeholder="emojicode" />
            </Field>
            <Field label="Path to git checkout">
              <input className={clsx(inputCls, "font-mono")} value={f.repo_path} onChange={set("repo_path")} placeholder="/Users/you/workspace/emojicode" />
            </Field>
            <Field label="Base branch (optional)">
              <input className={inputCls} value={f.base_branch} onChange={set("base_branch")} placeholder="master" />
            </Field>
          </>,
        )}
        {group(
          "GitHub (optional)",
          <>
            <Field label="Repository">
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
          </>,
        )}
        <ErrorBox error={create.error} />
        <Button variant="primary" size="lg" className="w-full justify-center" disabled={!f.slug || !f.repo_path || create.isPending}>
          Create project
        </Button>
      </form>
    </Page>
  );
}
