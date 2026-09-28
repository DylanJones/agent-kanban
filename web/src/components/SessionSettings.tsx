import clsx from "clsx";
import type { AgentDefinition } from "../api/client";
import { fieldCls } from "./ui";

/** An ACP session config option as the agent reports it. */
export type ConfigOption = {
  id: string;
  name: string;
  category?: string | null;
  type?: string;
  currentValue?: unknown;
  options?: { value?: string; name?: string; group?: string; options?: { value: string; name: string }[] }[];
};

/** Settings worth exposing per agent/role; `mode` is managed by the host/container mode settings. */
const SHOWN = new Set(["model", "thought_level", "model_config", "collaboration_mode"]);
const ORDER = ["model", "thought_level", "model_config", "collaboration_mode"];

export function agentOptions(agent?: AgentDefinition | null, only?: string[]): ConfigOption[] {
  const opts = (agent?.config_options as ConfigOption[] | null | undefined) ?? [];
  return opts
    .filter((o) => o.id !== "mode" && SHOWN.has(o.category ?? "") && (!only || only.includes(o.category ?? "")))
    .sort((a, b) => ORDER.indexOf(a.category ?? "") - ORDER.indexOf(b.category ?? ""));
}

export function choices(o: ConfigOption): { value: string; name: string }[] {
  return (o.options ?? []).flatMap((x) => (x.options ? x.options : x.value != null ? [{ value: x.value, name: x.name ?? x.value }] : []));
}

export function valueName(o: ConfigOption, v: unknown): string {
  if (typeof v === "boolean") return v ? "On" : "Off";
  return choices(o).find((c) => c.value === v)?.name ?? String(v ?? "");
}

export function settingLabel(o: ConfigOption): string {
  if (o.category === "model") return "Model";
  if (o.category === "thought_level") return "Effort";
  return o.name;
}

/**
 * One select per setting. `values` are explicit choices at this level; unset means "inherit",
 * shown as `inheritLabel(option)` (e.g. the agent default or the adapter's default).
 */
export function SettingSelects({
  options,
  values,
  onChange,
  inheritLabel,
  compact,
}: {
  options: ConfigOption[];
  values: Record<string, unknown>;
  onChange: (id: string, value: unknown | null) => void;
  inheritLabel: (o: ConfigOption) => string;
  compact?: boolean;
}) {
  return (
    <div className={clsx("flex flex-wrap gap-2", compact && "gap-1")}>
      {options.map((o) => {
        const v = values[o.id];
        const set = v !== undefined && v !== null;
        return (
          <label key={o.id} className="flex items-center gap-1 text-xs text-zinc-500">
            {!compact && <span>{settingLabel(o)}</span>}
            <select
              className={clsx(fieldCls, "py-0.5 text-xs", set && "border-blue-400 dark:border-blue-700")}
              value={set ? String(v) : ""}
              title={o.name}
              onChange={(e) => {
                const raw = e.target.value;
                if (raw === "") return onChange(o.id, null);
                onChange(o.id, o.type === "boolean" ? raw === "true" : raw);
              }}
            >
              <option value="">{inheritLabel(o)}</option>
              {o.type === "boolean" ? (
                <>
                  <option value="true">On</option>
                  <option value="false">Off</option>
                </>
              ) : (
                choices(o).map((c) => (
                  <option key={c.value} value={c.value}>
                    {c.name}
                  </option>
                ))
              )}
            </select>
          </label>
        );
      })}
    </div>
  );
}

/** "Opus 5.5 · High" from a run's effective settings. */
export function summarizeSettings(agent: AgentDefinition | undefined, values: Record<string, unknown> | null | undefined): string {
  if (!values) return "";
  const opts = agentOptions(agent, ["model", "thought_level"]);
  if (opts.length === 0) {
    return ["model", "effort", "reasoning_effort"].map((k) => values[k]).filter((v) => v != null && v !== "default").join(" · ");
  }
  return opts
    .map((o) => (values[o.id] != null ? valueName(o, values[o.id]) : null))
    .filter(Boolean)
    .join(" · ");
}
