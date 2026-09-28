import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";
import UsagePage from "./Usage";

const getMock = vi.fn();

vi.mock("../api/client", () => ({
  client: { GET: (...args: unknown[]) => getMock(...args) },
  unwrap: async <T,>(p: Promise<{ data?: T; error?: unknown; response: Response }>) => {
    const { data, error, response } = await p;
    if (!response.ok) throw new Error(JSON.stringify(error));
    return data;
  },
}));

function row(key: string) {
  return { key, runs: 1, total_tokens: 100, input_tokens: 10, cached_input_tokens: 0, cache_write_tokens: 0, output_tokens: 90, reasoning_tokens: 0, cost_usd: null };
}

function report(group_by: string, keys: string[]) {
  return {
    from: "2024-01-01T00:00:00Z",
    to: "2024-01-31T00:00:00Z",
    group_by,
    daily: [],
    rows: keys.map(row),
    totals: row("total"),
  };
}

function deferred<T>() {
  let resolve!: (v: T) => void;
  const promise = new Promise<T>((res) => {
    resolve = res;
  });
  return { promise, resolve };
}

function ok<T>(data: T) {
  return { data, error: undefined, response: new Response(null, { status: 200 }) };
}

function failed(message: string) {
  return { data: undefined, error: message, response: new Response(null, { status: 500 }) };
}

describe("UsagePage", () => {
  beforeEach(() => {
    getMock.mockReset();
  });

  afterEach(() => {
    cleanup();
  });

  function renderPage() {
    const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    return render(
      <QueryClientProvider client={qc}>
        <UsagePage />
      </QueryClientProvider>,
    );
  }

  test("switching to an uncached tab keeps the previous table mounted while it loads", async () => {
    const subscriptionReport = deferred<ReturnType<typeof ok>>();
    getMock.mockImplementation((path: string, opts: { params: { query: Record<string, string> } }) => {
      if (path === "/api/usage/subscriptions") return Promise.resolve(ok([]));
      const groupBy = opts.params.query.group_by;
      if (groupBy === "model") return Promise.resolve(ok(report("model", ["claude-opus"])));
      if (groupBy === "subscription") return subscriptionReport.promise;
      throw new Error(`unexpected group_by: ${groupBy}`);
    });

    renderPage();

    // The default (model) tab loads and renders its table.
    await screen.findByText("claude-opus");

    // Switching to an uncached tab (Subscription) must not unmount the existing table while
    // the new one is in flight -- that's what shrinks the page and jumps the scroll position.
    fireEvent.click(screen.getByRole("button", { name: "Subscription" }));
    await waitFor(() => expect(getMock).toHaveBeenCalledWith("/api/usage", { params: { query: expect.objectContaining({ group_by: "subscription" }) } }));
    expect(screen.queryByText("claude-opus")).not.toBeNull();

    // Once the new breakdown resolves, it replaces the old rows and the heading follows it.
    subscriptionReport.resolve(ok(report("subscription", ["claude"])));
    await screen.findByText("claude");
    expect(screen.queryByText("claude-opus")).toBeNull();
    expect(screen.queryByRole("columnheader", { name: "Subscription" })).not.toBeNull();
  });

  test("a failed uncached tab keeps the previous table mounted instead of collapsing the page", async () => {
    const subscriptionReport = deferred<ReturnType<typeof ok> | ReturnType<typeof failed>>();
    getMock.mockImplementation((path: string, opts: { params: { query: Record<string, string> } }) => {
      if (path === "/api/usage/subscriptions") return Promise.resolve(ok([]));
      const groupBy = opts.params.query.group_by;
      if (groupBy === "model") return Promise.resolve(ok(report("model", ["claude-opus"])));
      if (groupBy === "subscription") return subscriptionReport.promise;
      throw new Error(`unexpected group_by: ${groupBy}`);
    });

    renderPage();

    await screen.findByText("claude-opus");

    fireEvent.click(screen.getByRole("button", { name: "Subscription" }));
    await waitFor(() => expect(getMock).toHaveBeenCalledWith("/api/usage", { params: { query: expect.objectContaining({ group_by: "subscription" }) } }));

    // The uncached tab's request fails -- the previous table (and its heading) must stay
    // mounted, with the error surfaced explicitly, instead of collapsing the page.
    subscriptionReport.resolve(failed("boom"));
    await screen.findByText(/boom/);
    expect(screen.queryByText("claude-opus")).not.toBeNull();
    expect(screen.queryByRole("columnheader", { name: "Model" })).not.toBeNull();
  });
});
