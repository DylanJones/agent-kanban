import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { useState } from "react";
import { MemoryRouter } from "react-router";
import { afterEach, beforeAll, beforeEach, describe, expect, test, vi } from "vitest";
import { MAX_TOASTS, NavDrawer, RunToasts } from "./Layout";

// jsdom does no layout, so `offsetParent` (which the drawer's focus trap uses to skip hidden
// controls) is always null; stand in with "has a parent" so mounted, non-`display:none` elements
// still count as visible.
beforeAll(() => {
  Object.defineProperty(HTMLElement.prototype, "offsetParent", {
    get() {
      return this.parentElement;
    },
    configurable: true,
  });
});

const getMock = vi.fn();
vi.mock("../api/client", async () => {
  const actual = await vi.importActual<typeof import("../api/client")>("../api/client");
  return { ...actual, client: { GET: (...args: unknown[]) => getMock(...args) } };
});

function ok<T>(data: T) {
  return Promise.resolve({ data, error: undefined, response: new Response(null, { status: 200 }) });
}

function mockEndpoints() {
  getMock.mockImplementation((path: string) => {
    if (path === "/api/projects")
      return ok([
        { slug: "demo", name: "Demo", github_repo: "someone/demo" },
        { slug: "other", name: "Other" },
      ]);
    if (path === "/api/settings") return ok({ scheduler_enabled: false, max_concurrent_runs: 3 });
    if (path === "/api/runs") return ok([]);
    throw new Error(`unexpected path: ${path}`);
  });
}

// The drawer's ProjectSelect/SchedulerControl children add extra focusable controls between the
// title link and the "API docs" link, so real DOM order — not a hardcoded index — decides which
// element is first/last for the wrap-around trap below.
function renderDrawer(open: boolean, onClose: () => void, initialEntries = ["/p/demo"]) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <MemoryRouter initialEntries={initialEntries}>
        <NavDrawer open={open} onClose={onClose} current="demo" isHuman={false} />
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

function TriggerAndDrawer({ initialEntries = ["/p/demo"] }: { initialEntries?: string[] }) {
  const [open, setOpen] = useState(false);
  return (
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <button onClick={() => setOpen(true)}>open menu</button>
      <MemoryRouter initialEntries={initialEntries}>
        <NavDrawer open={open} onClose={() => setOpen(false)} current="demo" isHuman={false} />
      </MemoryRouter>
    </QueryClientProvider>
  );
}

describe("NavDrawer", () => {
  beforeEach(() => {
    getMock.mockReset();
    mockEndpoints();
  });
  afterEach(cleanup);

  test("closed drawer is inert (excluded from keyboard focus)", () => {
    renderDrawer(false, () => {});
    // aria-hidden removes it from the accessibility tree, so query past that with `hidden: true`.
    const titleLink = screen.getByRole("link", { name: /agent-kanban/, hidden: true });
    expect(titleLink.closest("[inert]")).not.toBeNull();
  });

  test("opening moves focus to the first focusable control (the title link, not Close)", async () => {
    render(<TriggerAndDrawer />);
    const trigger = screen.getByRole("button", { name: "open menu" });
    trigger.focus();
    fireEvent.click(trigger);
    await screen.findByRole("link", { name: /agent-kanban/ });
    expect(document.activeElement).toBe(screen.getByRole("link", { name: /agent-kanban/ }));
  });

  test("wrapper is no longer inert once open", async () => {
    render(<TriggerAndDrawer />);
    fireEvent.click(screen.getByRole("button", { name: "open menu" }));
    const titleLink = await screen.findByRole("link", { name: /agent-kanban/ });
    expect(titleLink.closest("[inert]")).toBeNull();
  });

  test("Shift+Tab from the first control wraps to the last, and Tab from the last wraps to the first", async () => {
    render(<TriggerAndDrawer />);
    fireEvent.click(screen.getByRole("button", { name: "open menu" }));
    const titleLink = await screen.findByRole("link", { name: /agent-kanban/ });
    const apiDocs = screen.getByRole("link", { name: /API docs/ });
    expect(document.activeElement).toBe(titleLink);

    fireEvent.keyDown(window, { key: "Tab", shiftKey: true });
    expect(document.activeElement).toBe(apiDocs);

    fireEvent.keyDown(window, { key: "Tab" });
    expect(document.activeElement).toBe(titleLink);
  });

  test("Tab pulls focus back into the panel if it lands outside while open", async () => {
    render(<TriggerAndDrawer />);
    const trigger = screen.getByRole("button", { name: "open menu" });
    fireEvent.click(trigger);
    await screen.findByRole("link", { name: /agent-kanban/ });

    // The trigger sits outside the panel — simulates focus escaping (or never having moved in).
    trigger.focus();
    expect(document.activeElement).toBe(trigger);

    fireEvent.keyDown(window, { key: "Tab" });
    expect(document.activeElement).toBe(screen.getByRole("link", { name: /agent-kanban/ }));

    trigger.focus();
    fireEvent.keyDown(window, { key: "Tab", shiftKey: true });
    expect(document.activeElement).toBe(screen.getByRole("link", { name: /API docs/ }));
  });

  test("Escape closes the drawer and restores focus to the trigger", async () => {
    render(<TriggerAndDrawer />);
    const trigger = screen.getByRole("button", { name: "open menu" });
    trigger.focus();
    fireEvent.click(trigger);
    await screen.findByRole("link", { name: /agent-kanban/ });

    fireEvent.keyDown(window, { key: "Escape" });
    expect(document.activeElement).toBe(trigger);
    expect(screen.getByRole("link", { name: /agent-kanban/, hidden: true }).closest("[inert]")).not.toBeNull();
  });

  test("clicking Close menu closes the drawer and restores focus", async () => {
    render(<TriggerAndDrawer />);
    const trigger = screen.getByRole("button", { name: "open menu" });
    trigger.focus();
    fireEvent.click(trigger);
    await screen.findByRole("button", { name: "Close menu" });

    fireEvent.click(screen.getByRole("button", { name: "Close menu" }));
    expect(document.activeElement).toBe(trigger);
  });

  test("clicking the backdrop closes the drawer and restores focus", async () => {
    const { container } = render(<TriggerAndDrawer />);
    const trigger = screen.getByRole("button", { name: "open menu" });
    trigger.focus();
    fireEvent.click(trigger);
    await screen.findByRole("link", { name: /agent-kanban/ });

    const backdrop = container.querySelector(".bg-black\\/40");
    expect(backdrop).not.toBeNull();
    fireEvent.click(backdrop as Element);
    expect(document.activeElement).toBe(trigger);
  });

  test("navigating via a nav link closes the drawer", async () => {
    const onClose = vi.fn();
    renderDrawer(true, onClose);
    fireEvent.click(await screen.findByRole("link", { name: "Runs" }));
    expect(onClose).toHaveBeenCalled();
  });

  test("clicking the title link while already on that route still dismisses the drawer", async () => {
    const onClose = vi.fn();
    renderDrawer(true, onClose, ["/p/demo"]);
    fireEvent.click(await screen.findByRole("link", { name: /agent-kanban/ }));
    expect(onClose).toHaveBeenCalled();
  });

  test("the project switcher opens a menu of projects, and Escape closes only the menu", async () => {
    const onClose = vi.fn();
    renderDrawer(true, onClose);
    const switcher = await screen.findByRole("button", { name: /Demo/ });
    expect(switcher.getAttribute("aria-expanded")).toBe("false");

    fireEvent.click(switcher);
    const menu = screen.getByRole("menu", { name: "Projects" });
    const current = within(menu).getByRole("menuitemradio", { name: /Demo/ });
    expect(current.getAttribute("aria-checked")).toBe("true");
    expect(within(menu).getByRole("menuitemradio", { name: /Other/ }).getAttribute("aria-checked")).toBe("false");
    expect(within(menu).getByRole("menuitem", { name: /Add project/ })).toBeTruthy();
    expect(document.activeElement).toBe(current);

    fireEvent.keyDown(current, { key: "ArrowDown" });
    expect(document.activeElement).toBe(within(menu).getByRole("menuitemradio", { name: /Other/ }));

    fireEvent.keyDown(document.activeElement!, { key: "Escape" });
    expect(screen.queryByRole("menu")).toBeNull();
    expect(document.activeElement).toBe(switcher);
    expect(onClose).not.toHaveBeenCalled();
  });

  test("choosing another project navigates there", async () => {
    const onClose = vi.fn();
    renderDrawer(true, onClose);
    fireEvent.click(await screen.findByRole("button", { name: /Demo/ }));
    fireEvent.click(screen.getByRole("menuitemradio", { name: /Other/ }));
    expect(screen.queryByRole("menu")).toBeNull();
    // The drawer closes on any route change, so reaching the other project's board shows up as a close.
    expect(onClose).toHaveBeenCalled();
  });
});

describe("RunToasts", () => {
  let active: { id: number }[];
  const run = (id: number) => ({ id, role: "fix", status: "succeeded", issue: id, issue_title: `Issue ${id}`, agent: "codex", error: null });

  beforeEach(() => {
    getMock.mockReset();
    active = Array.from({ length: 8 }, (_, i) => run(i + 1));
    getMock.mockImplementation((path: string, opts?: { params?: { path?: { id?: number } } }) => {
      if (path === "/api/runs") return ok(active);
      if (path === "/api/runs/{id}") return ok(run(opts!.params!.path!.id!));
      throw new Error(`unexpected path: ${path}`);
    });
  });
  afterEach(cleanup);

  async function finishAll() {
    const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(
      <QueryClientProvider client={qc}>
        <MemoryRouter>
          <RunToasts />
        </MemoryRouter>
      </QueryClientProvider>,
    );
    await waitFor(() => expect(document.title).toBe("(8 running) agent-kanban"));
    active = [];
    await act(() => qc.refetchQueries({ queryKey: ["runs", "active"] }));
    await screen.findByRole("link", { name: /more runs finished/ });
  }

  test("a burst of finished runs shows a bounded stack plus a summary link", async () => {
    await finishAll();
    const region = screen.getByRole("status", { name: "Run notifications" });
    const toasts = within(region).getAllByRole("link", { name: /run succeeded/ });
    expect(toasts).toHaveLength(MAX_TOASTS);
    // The newest runs stay visible; the rest are summarised.
    expect(toasts.map((t) => t.getAttribute("href"))).toEqual(["/runs/6", "/runs/7", "/runs/8"]);
    const more = within(region).getByRole("link", { name: `+${8 - MAX_TOASTS} more runs finished` });
    expect(more.getAttribute("href")).toBe("/runs");
  });

  test("toasts sit at the top on phones and below the menu drawer and dialogs (z-50)", async () => {
    await finishAll();
    const cls = screen.getByRole("status", { name: "Run notifications" }).className;
    expect(cls).toMatch(/(^| )top-\[/);
    expect(cls).not.toMatch(/(^| )bottom-/);
    expect(cls).toMatch(/(^| )z-40( |$)/);
  });

  test("each toast can be dismissed, which reveals the next one", async () => {
    await finishAll();
    fireEvent.click(screen.getByRole("button", { name: "Dismiss run #8 notification" }));
    const hrefs = screen.getAllByRole("link", { name: /run succeeded/ }).map((t) => t.getAttribute("href"));
    expect(hrefs).toEqual(["/runs/5", "/runs/6", "/runs/7"]);
    expect(screen.getByRole("link", { name: "+4 more runs finished" })).toBeTruthy();
  });
});
