import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, test } from "vitest";
import { RunUsageChips, contextLabel } from "./RunUsage";

afterEach(cleanup);

describe("contextLabel", () => {
  test("formats used / capacity with a percentage", () => {
    expect(contextLabel(46012, 258400)).toBe("context 46k / 258k (18%)");
  });
  test("omits capacity when the adapter doesn't report one", () => {
    expect(contextLabel(1200, undefined)).toBe("context 1.2k");
  });
  test("returns null without a used figure", () => {
    expect(contextLabel(undefined, 258400)).toBeNull();
  });
});

describe("RunUsageChips", () => {
  // Run #240 from issue #35: 305k cumulative tokens next to a 46k/258k context.
  const run240 = { totalTokens: 304631, models: ["gpt-6-astra"], context: { used: 46012, size: 258400 } };

  test("labels the cumulative total and the context occupancy distinctly", () => {
    render(<RunUsageChips {...run240} />);
    expect(screen.getByText("305k tokens total")).toBeTruthy();
    expect(screen.getByText("context 46k / 258k (18%)")).toBeTruthy();
  });

  test("explains both figures on tap, not only on hover", () => {
    render(<RunUsageChips {...run240} />);
    const btn = screen.getByRole("button", { name: /what do these token numbers mean/i });
    expect(btn.getAttribute("aria-expanded")).toBe("false");
    expect(screen.queryByText(/summed/)).toBeNull();
    fireEvent.click(btn);
    expect(btn.getAttribute("aria-expanded")).toBe("true");
    expect(screen.getByText(/including cache reads\) plus its output, summed/)).toBeTruthy();
    expect(screen.getByText(/out of the window's capacity/)).toBeTruthy();
    expect(screen.getByText(/Models: gpt-6-astra/)).toBeTruthy();
  });

  test("renders nothing without usage", () => {
    const { container } = render(<RunUsageChips totalTokens={0} models={[]} />);
    expect(container.innerHTML).toBe("");
  });
});
