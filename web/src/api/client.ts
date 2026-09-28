import createClient from "openapi-fetch";
import type { components, paths } from "./schema";

export type S = components["schemas"];
export type Board = S["Board"];
export type Card = S["Card"];
export type IssueDetail = S["IssueDetail"];
export type IssueState = S["IssueState"];
export type Hold = S["Hold"];
export type PullDetail = S["PullDetail"];
export type Thread = S["ThreadWithComments"];
export type Comment = S["Comment"];
export type RunView = S["RunView"];
export type RunEvent = S["RunEvent"];
export type AgentDefinition = S["AgentDefinition"];
export type LimitGroup = S["LimitGroup"];
export type Project = S["Project"];
export type Role = S["Role"];

export const client = createClient<paths>({ baseUrl: "", credentials: "include" });

export class ApiError extends Error {
  status: number;
  problem: S["Problem"] | undefined;
  constructor(status: number, problem?: S["Problem"]) {
    super(problem?.detail || problem?.title || `HTTP ${status}`);
    this.status = status;
    this.problem = problem;
  }
}

/** Unwrap an openapi-fetch result, throwing ApiError on failure. */
export async function unwrap<T>(p: Promise<{ data?: T; error?: unknown; response: Response }>): Promise<T> {
  const { data, error, response } = await p;
  if (!response.ok) {
    throw new ApiError(response.status, error as S["Problem"] | undefined);
  }
  return data as T;
}

/** Untyped JSON helper for the few places the typed client is awkward. */
export async function api<T = unknown>(method: string, path: string, body?: unknown): Promise<T> {
  const res = await fetch(path, {
    method,
    credentials: "include",
    headers: body !== undefined ? { "Content-Type": "application/json" } : undefined,
    body: body !== undefined ? JSON.stringify(body) : undefined,
  });
  const text = await res.text();
  const json = text ? JSON.parse(text) : undefined;
  if (!res.ok) throw new ApiError(res.status, json);
  return json as T;
}

export const STATE_LABEL: Record<IssueState, string> = {
  triage: "Triage",
  backlog: "Backlog",
  ready: "Ready",
  in_progress: "In progress",
  changes_requested: "Changes requested",
  merge_conflict: "Merge conflict",
  in_review: "In review",
  ready_to_merge: "Ready to merge",
  done: "Done",
  closed: "Closed",
};

export const COLUMN_STATES: Record<string, IssueState[]> = {
  backlog: ["triage", "backlog", "ready"],
  in_progress: ["in_progress", "changes_requested", "merge_conflict"],
  in_review: ["in_review", "ready_to_merge"],
  done: ["done", "closed"],
};

export const HOLD_LABEL: Record<Hold, string> = {
  needs_decision: "Needs decision",
  stalled: "Stalled",
  paused: "Paused",
};

export const ROLE_LABEL: Record<Role, string> = {
  triage: "Triage",
  fix: "Fix",
  review: "Review",
  merge_prep: "Merge prep",
};
