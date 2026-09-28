import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, render } from "@testing-library/react";
import { afterEach, describe, expect, test } from "vitest";
import type { Thread } from "../api/client";
import { ThreadView } from "./PullPage";

afterEach(cleanup);

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
