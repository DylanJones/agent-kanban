import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { MemoryRouter, Route, Routes } from "react-router";
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";
import type { Thread } from "../api/client";
import PullPage, { ThreadView } from "./PullPage";

function longThread(): Thread {
  const path = `src/${"long_name_".repeat(12)}.tsx`;
  return {
    id: 1,
    path,
    line: 42,
    side: "RIGHT",
    severity: "blocking",
    resolved: false,
    outdated: false,
    created_at: "2026-01-01T00:00:00Z",
    comments: [
      {
        id: 1,
        author_kind: "human",
        author_name: "reviewer",
        body: "This needs a fix.",
        kind: "comment",
        created_at: "2026-01-01T00:00:00Z",
        updated_at: "2026-01-01T00:00:00Z",
      },
    ],
  } as Thread;
}

describe("ThreadView compact header", () => {
  afterEach(cleanup);

  test("the long unbroken path is CSS-truncatable instead of forcing the row to grow", () => {
    const qc = new QueryClient();
    const { container } = render(
      <QueryClientProvider client={qc}>
        <ThreadView t={longThread()} compact />
      </QueryClientProvider>,
    );
    const code = container.querySelector("code");
    expect(code).not.toBeNull();
    expect(code?.textContent).toContain("long_name_");
    // A flex child only truncates instead of overflowing its row when it can shrink
    // (min-w-0) and clips its own text (truncate) — without both, one long unbroken
    // token in the path widens the whole row and the page scrolls sideways.
    expect(code?.className).toContain("min-w-0");
    expect(code?.className).toContain("truncate");

    const button = container.querySelector("button");
    expect(button?.className).not.toContain("overflow-x-auto");
  });
});

const getMock = vi.fn();

vi.mock("../api/client", async () => {
  const actual = await vi.importActual<typeof import("../api/client")>("../api/client");
  return {
    ...actual,
    client: { GET: (...args: unknown[]) => getMock(...args) },
    api: vi.fn(),
  };
});

function pull(overrides: Partial<Record<string, unknown>> = {}) {
  return {
    number: 1,
    title: "Fix example",
    body: "",
    branch: "fix",
    base_branch: "main",
    author_kind: "agent",
    author_name: "codex",
    state: "open",
    head_sha: "abc123",
    conflict_files: [],
    has_conflicts: false,
    created_at: "2024-01-01T00:00:00Z",
    default_merge_message: "Fix example\n\nCloses #15",
    issues: [15],
    comments: [],
    reviews: [],
    threads: [],
    url: "/pulls/1",
    ...overrides,
  };
}

function ok<T>(data: T) {
  return { data, error: undefined, response: new Response(null, { status: 200 }) };
}

function mockEndpoints({ prOverrides = {}, projectOverrides = {} }: { prOverrides?: Record<string, unknown>; projectOverrides?: Record<string, unknown> } = {}) {
  getMock.mockImplementation((path: string) => {
    if (path === "/api/projects/{p}/pulls/{n}") return Promise.resolve(ok(pull(prOverrides)));
    if (path === "/api/projects/{p}/pulls/{n}/mergeability") return Promise.resolve(ok({ mergeable: true, has_conflicts: false, blockers: [] }));
    if (path === "/api/projects/{p}") return Promise.resolve(ok({ merge_strategy: "merge", commit_msg_regex: "^🐛", ...projectOverrides }));
    throw new Error(`unexpected path: ${path}`);
  });
}

function renderPage() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <MemoryRouter initialEntries={["/p/demo/pulls/1"]}>
        <Routes>
          <Route path="/p/:slug/pulls/:n" element={<PullPage />} />
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

describe("PullPage merge box message validation", () => {
  beforeEach(() => {
    getMock.mockReset();
  });

  afterEach(() => {
    cleanup();
  });

  test("blank message is validated against the generated fallback, not skipped", async () => {
    // default_merge_message "Fix example..." doesn't match the project's commit_msg_regex "^🐛".
    mockEndpoints();
    renderPage();

    await screen.findByText("Ready to merge");
    const textarea = screen.getByPlaceholderText(/Leave blank to generate/);
    fireEvent.change(textarea, { target: { value: "" } });
    expect(textarea).toHaveProperty("value", "");

    const button = screen.getByRole("button", { name: /Merge into/ });
    await waitFor(() => expect(button).toHaveProperty("disabled", true));
    expect(screen.getByText(/First line must match/)).not.toBeNull();
  });

  test("blank message is accepted when the generated fallback matches the regex", async () => {
    mockEndpoints({ prOverrides: { default_merge_message: "🐛 Fix example\n\nCloses #15" } });
    renderPage();

    await screen.findByText("Ready to merge");
    const textarea = screen.getByPlaceholderText(/Leave blank to generate/);
    fireEvent.change(textarea, { target: { value: "" } });
    expect(textarea).toHaveProperty("value", "");

    const button = screen.getByRole("button", { name: /Merge into/ });
    await waitFor(() => expect(button).toHaveProperty("disabled", false));
    expect(screen.queryByText(/First line must match/)).toBeNull();
  });

  test("whitespace-only message is treated as blank and validated against the fallback", async () => {
    mockEndpoints();
    renderPage();

    await screen.findByText("Ready to merge");
    const textarea = screen.getByPlaceholderText(/Leave blank to generate/);
    fireEvent.change(textarea, { target: { value: "Fix example\n\nCloses #15" } });
    fireEvent.change(textarea, { target: { value: "   \n  " } });

    const button = screen.getByRole("button", { name: /Merge into/ });
    await waitFor(() => expect(button).toHaveProperty("disabled", true));
  });

  test("whitespace-only message is accepted when the generated fallback matches the regex", async () => {
    mockEndpoints({ prOverrides: { default_merge_message: "🐛 Fix example\n\nCloses #15" } });
    renderPage();

    await screen.findByText("Ready to merge");
    const textarea = screen.getByPlaceholderText(/Leave blank to generate/);
    fireEvent.change(textarea, { target: { value: "   \n  " } });

    const button = screen.getByRole("button", { name: /Merge into/ });
    await waitFor(() => expect(button).toHaveProperty("disabled", false));
    expect(screen.queryByText(/First line must match/)).toBeNull();
  });

  test("a custom message is validated as typed", async () => {
    mockEndpoints();
    renderPage();

    await screen.findByText("Ready to merge");
    const textarea = screen.getByPlaceholderText(/Leave blank to generate/);
    fireEvent.change(textarea, { target: { value: "🐛 Custom fix\n\nCloses #15" } });

    const button = screen.getByRole("button", { name: /Merge into/ });
    await waitFor(() => expect(button).toHaveProperty("disabled", false));

    fireEvent.change(textarea, { target: { value: "Custom fix without emoji" } });
    await waitFor(() => expect(button).toHaveProperty("disabled", true));
  });
});
