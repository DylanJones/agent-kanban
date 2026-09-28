import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";

// `client` (openapi-fetch) needs a real absolute-URL-resolving `fetch`/`Request`, which this
// jsdom test environment doesn't provide for relative paths; mock the thin wrapper instead of
// fighting that, so `Server.tsx` can be exercised exactly as it calls `client.GET`/`client.POST`.
const clientGet = vi.fn();
const clientPost = vi.fn();
vi.mock("../api/client", () => ({
  client: { GET: (...args: unknown[]) => clientGet(...args), POST: (...args: unknown[]) => clientPost(...args) },
  unwrap: async <T,>(p: Promise<{ data?: T; error?: unknown; response: { ok: boolean; status: number } }>) => {
    const { data, error, response } = await p;
    if (!response.ok) throw Object.assign(new Error("api error"), { status: response.status, problem: error });
    return data as T;
  },
}));

const { default: ServerPage } = await import("./Server");

function ok<T>(data: T) {
  return Promise.resolve({ data, error: undefined, response: { ok: true, status: 200 } });
}

function buildStatus(overrides: Record<string, unknown> = {}) {
  return {
    active_runs: 0,
    build_sha: "abc123",
    build_time: "2026-01-01T00:00:00Z",
    commits_behind: 0,
    dirty: false,
    dispatch_paused: false,
    head_sha: "abc123",
    head_summary: "initial",
    instance_id: "inst-1",
    on_main: true,
    restart_pending: false,
    ...overrides,
  };
}

function job(overrides: Record<string, unknown> = {}) {
  return {
    id: 1,
    kind: "server.build",
    status: "queued",
    log: "",
    payload: {},
    attempts: 0,
    created_at: "2026-01-01T00:00:00Z",
    error: null,
    finished_at: null,
    project_id: null,
    ...overrides,
  };
}

function renderPage() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  render(
    <QueryClientProvider client={qc}>
      <ServerPage />
    </QueryClientProvider>,
  );
  return qc;
}

describe("ServerPage", () => {
  beforeEach(() => {
    clientGet.mockReset();
    clientPost.mockReset();
  });

  afterEach(() => {
    cleanup();
  });

  test("a build that fails re-enables the buttons and surfaces the error, instead of getting stuck on 'Restarting…'", async () => {
    let jobs: ReturnType<typeof job>[] = [];
    clientGet.mockImplementation((path: string) => {
      if (path === "/api/build-status") return ok(buildStatus());
      if (path === "/api/jobs") return ok(jobs);
      throw new Error(`unexpected GET ${path}`);
    });
    clientPost.mockImplementation(() => {
      // The (fake) build has already failed by the time anything re-polls the job.
      jobs = [job({ id: 7, status: "failed", error: "cargo build failed: E0432" })];
      return ok(job({ id: 7, status: "queued" }));
    });

    renderPage();
    const button = await screen.findByRole("button", { name: /Rebuild & restart now/ });
    await act(async () => {
      fireEvent.click(button);
    });

    await waitFor(() => expect(screen.getByText(/cargo build failed: E0432/)).toBeTruthy());
    // The buttons must come back so the user can retry, not stay stuck on "Restarting…" forever
    // even though the server never actually started restarting.
    expect(screen.getByRole("button", { name: /Rebuild & restart now/ })).toBeTruthy();
    expect(screen.queryByText(/Restarting…/)).toBeNull();
  });

  test("a completion write that never lands surfaces via deploy_error instead of waiting forever", async () => {
    // The job row itself is stuck `running` forever with no `error` (e.g. every DB write for this
    // deploy failed, not just the one for the `succeeded` status) — the only way this page can
    // learn the deploy failed is `BuildStatus.deploy_error`, which is in-memory on the server and
    // so can't fail to persist the way the job row's own columns can (thread #122 on issue #12).
    let status = buildStatus();
    let jobs: ReturnType<typeof job>[] = [];
    clientGet.mockImplementation((path: string) => {
      if (path === "/api/build-status") return ok(status);
      if (path === "/api/jobs") return ok(jobs);
      throw new Error(`unexpected GET ${path}`);
    });
    clientPost.mockImplementation(() => {
      jobs = [job({ id: 13, status: "running" })];
      return ok(job({ id: 13, status: "queued" }));
    });

    const qc = renderPage();
    const button = await screen.findByRole("button", { name: /Rebuild & restart now/ });
    await act(async () => {
      fireEvent.click(button);
    });

    await waitFor(() => expect(screen.getByText(/Restarting…/)).toBeTruthy());

    status = buildStatus({ deploy_error: "build succeeded but its result could not be durably recorded" });
    await act(async () => {
      await qc.invalidateQueries({ queryKey: ["build-status"] });
    });

    await waitFor(() => expect(screen.getByText(/could not be durably recorded/)).toBeTruthy());
    // The buttons must come back so the user can retry, not stay stuck on "Restarting…" forever
    // waiting for a job row that will never leave `running`.
    expect(screen.getByRole("button", { name: /Rebuild & restart now/ })).toBeTruthy();
    expect(screen.queryByText(/Restarting…/)).toBeNull();
  });

  test("a same-commit rebuild still reloads once the server instance actually changes", async () => {
    const reload = vi.fn();
    Object.defineProperty(window, "location", { value: { ...window.location, reload }, writable: true });

    let status = buildStatus({ instance_id: "inst-1", restart_pending: false });
    let jobs: ReturnType<typeof job>[] = [];
    clientGet.mockImplementation((path: string) => {
      if (path === "/api/build-status") return ok(status);
      if (path === "/api/jobs") return ok(jobs);
      throw new Error(`unexpected GET ${path}`);
    });
    clientPost.mockImplementation(() => {
      jobs = [job({ id: 9, status: "succeeded" })];
      return ok(job({ id: 9, status: "queued" }));
    });

    const qc = renderPage();
    const button = await screen.findByRole("button", { name: /Rebuild & restart now/ });
    await act(async () => {
      fireEvent.click(button);
    });

    // The build succeeded (no new commits, so `build_sha` doesn't change) and the still-running
    // old process now reports it's about to restart.
    status = buildStatus({ instance_id: "inst-1", restart_pending: true });
    await act(async () => {
      await qc.invalidateQueries({ queryKey: ["build-status"] });
    });
    await waitFor(() => expect(screen.getByText(/Restarting…/)).toBeTruthy());
    expect(reload).not.toHaveBeenCalled();

    // The restart completes: a new process, with the very same `build_sha`, but a new
    // `instance_id`. Comparing `build_sha` alone would never notice this restart happened.
    status = buildStatus({ instance_id: "inst-2", restart_pending: false });
    await act(async () => {
      await qc.invalidateQueries({ queryKey: ["build-status"] });
    });

    await waitFor(() => expect(reload).toHaveBeenCalled());
  });

  test("a retry after a failed build isn't knocked off tracking by the previous attempt's stale error", async () => {
    const reload = vi.fn();
    Object.defineProperty(window, "location", { value: { ...window.location, reload }, writable: true });

    let status = buildStatus({ instance_id: "inst-1" });
    let jobs: ReturnType<typeof job>[] = [];
    clientGet.mockImplementation((path: string) => {
      if (path === "/api/build-status") return ok(status);
      if (path === "/api/jobs") return ok(jobs);
      throw new Error(`unexpected GET ${path}`);
    });

    // First attempt: a completion write that never lands (thread #122), surfaced only via
    // `deploy_error`. This leaves `deployError` latched in local state and the buttons re-enabled.
    clientPost.mockImplementationOnce(() => {
      jobs = [job({ id: 13, status: "running" })];
      return ok(job({ id: 13, status: "queued" }));
    });

    const qc = renderPage();
    const button = await screen.findByRole("button", { name: /Rebuild & restart now/ });
    await act(async () => {
      fireEvent.click(button);
    });
    await waitFor(() => expect(screen.getByText(/Restarting…/)).toBeTruthy());

    status = buildStatus({ instance_id: "inst-1", deploy_error: "build succeeded but its result could not be durably recorded" });
    await act(async () => {
      await qc.invalidateQueries({ queryKey: ["build-status"] });
    });
    await waitFor(() => expect(screen.getByText(/could not be durably recorded/)).toBeTruthy());

    // Retry: the server admits a new job and clears its own `deploy_error` synchronously as part
    // of that (before this POST even resolves) — but the `build-status` value already cached here
    // predates that and still carries the stale error from the previous attempt.
    clientPost.mockImplementationOnce(() => {
      jobs = [job({ id: 14, status: "queued" })];
      status = buildStatus({ instance_id: "inst-1" });
      return ok(job({ id: 14, status: "queued" }));
    });
    const retryButton = await screen.findByRole("button", { name: /Rebuild & restart now/ });
    await act(async () => {
      fireEvent.click(retryButton);
    });

    // The stale cached error must not resurrect and knock the new attempt off its tracking.
    expect(screen.queryByText(/could not be durably recorded/)).toBeNull();
    await waitFor(() => expect(screen.getByText(/Restarting…/)).toBeTruthy());

    // The retry succeeds and the server restarts into a new instance.
    jobs = [job({ id: 14, status: "succeeded" })];
    status = buildStatus({ instance_id: "inst-2", deploy_error: null });
    await act(async () => {
      await qc.invalidateQueries({ queryKey: ["build-status"] });
      await qc.invalidateQueries({ queryKey: ["jobs", "server.build"] });
    });

    await waitFor(() => expect(reload).toHaveBeenCalled());
  });

  test("a stale status response landing in the same instant as a retry's admission isn't mistaken for its error", async () => {
    // Reproduces the tied-clock boundary from review run #204: a `build-status` response from
    // *before* the retry (still carrying the previous attempt's `deploy_error`) can finish with
    // the exact same `Date.now()` millisecond as the moment the retry starts being awaited. `>=`
    // treated that tie as "fresh enough" and let the stale error resurrect; only strict `>`
    // rejects it. `qc.setQueryData`'s `updatedAt` option pins the timestamp exactly, so the test
    // doesn't depend on real clock timing to hit the tie.
    const reload = vi.fn();
    Object.defineProperty(window, "location", { value: { ...window.location, reload }, writable: true });

    let status = buildStatus({ instance_id: "inst-1" });
    let jobs: ReturnType<typeof job>[] = [];
    clientGet.mockImplementation((path: string) => {
      if (path === "/api/build-status") return ok(status);
      if (path === "/api/jobs") return ok(jobs);
      throw new Error(`unexpected GET ${path}`);
    });

    // First attempt: a completion write that never lands (thread #122), surfaced only via
    // `deploy_error`. This leaves `deployError` latched in local state.
    clientPost.mockImplementationOnce(() => {
      jobs = [job({ id: 13, status: "running" })];
      return ok(job({ id: 13, status: "queued" }));
    });

    const qc = renderPage();
    const firstButton = await screen.findByRole("button", { name: /Rebuild & restart now/ });
    await act(async () => {
      fireEvent.click(firstButton);
    });
    await waitFor(() => expect(screen.getByText(/Restarting…/)).toBeTruthy());

    const staleStatus = buildStatus({ instance_id: "inst-1", deploy_error: "build succeeded but its result could not be durably recorded" });
    status = staleStatus;
    await act(async () => {
      await qc.invalidateQueries({ queryKey: ["build-status"] });
    });
    await waitFor(() => expect(screen.getByText(/could not be durably recorded/)).toBeTruthy());

    const FIXED = 1_700_000_000_000;
    const dateNowSpy = vi.spyOn(Date, "now").mockReturnValue(FIXED);
    try {
      clientPost.mockImplementationOnce(() => {
        jobs = [job({ id: 14, status: "queued" })];
        status = buildStatus({ instance_id: "inst-1" });
        return ok(job({ id: 14, status: "queued" }));
      });
      const retryButton = await screen.findByRole("button", { name: /Rebuild & restart now/ });
      await act(async () => {
        fireEvent.click(retryButton);
      });

      // Model the stale in-flight response (issued before the retry) landing right after
      // admission, at the exact same `Date.now()` tick as `awaitingSince`.
      await act(async () => {
        qc.setQueryData(["build-status"], staleStatus, { updatedAt: FIXED });
      });
    } finally {
      dateNowSpy.mockRestore();
    }

    // The tied-timestamp stale error must not resurrect and knock the retry off its tracking.
    expect(screen.queryByText(/could not be durably recorded/)).toBeNull();
    await waitFor(() => expect(screen.getByText(/Restarting…/)).toBeTruthy());

    // The retry succeeds and the server restarts into a new instance.
    jobs = [job({ id: 14, status: "succeeded" })];
    status = buildStatus({ instance_id: "inst-2", deploy_error: null });
    await act(async () => {
      await qc.invalidateQueries({ queryKey: ["build-status"] });
      await qc.invalidateQueries({ queryKey: ["jobs", "server.build"] });
    });

    await waitFor(() => expect(reload).toHaveBeenCalled());
  });
});
