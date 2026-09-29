import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { useState } from "react";
import { afterEach, describe, expect, test, vi } from "vitest";
import { Markdown, Segmented, Switch, fieldCls, fieldSmCls, inputCls } from "./ui";

/**
 * iOS Safari zooms the page in on focus when an editable element computes to
 * a font-size under 16px, so form inputs must default to text-base (16px)
 * and can only drop to text-sm at a `sm:` breakpoint or wider.
 */
function assertNoMobileZoomFont(cls: string) {
  expect(cls).toMatch(/(?:^|\s)text-base(?:\s|$)/);
  expect(cls).not.toMatch(/(?:^|\s)text-sm(?:\s|$)/);
}

describe("shared form field classes", () => {
  test("inputCls does not trigger iOS Safari focus zoom on phones", () => {
    assertNoMobileZoomFont(inputCls);
  });

  test("fieldCls does not trigger iOS Safari focus zoom on phones", () => {
    assertNoMobileZoomFont(fieldCls);
  });

  test("fieldSmCls does not trigger iOS Safari focus zoom on phones", () => {
    assertNoMobileZoomFont(fieldSmCls);
  });
});

describe("Switch", () => {
  afterEach(cleanup);

  test("is a named switch that reports and flips its state", () => {
    const onChange = vi.fn();
    render(<Switch checked={false} onChange={onChange} label="Scheduler" />);
    const sw = screen.getByRole("switch", { name: "Scheduler" });
    expect(sw.getAttribute("aria-checked")).toBe("false");
    fireEvent.click(sw);
    expect(onChange).toHaveBeenCalledWith(true);
  });

  test("never submits an enclosing form", () => {
    const onSubmit = vi.fn((e: { preventDefault: () => void }) => e.preventDefault());
    render(
      <form onSubmit={onSubmit}>
        <Switch checked onChange={() => {}} label="Follow" />
      </form>,
    );
    fireEvent.click(screen.getByRole("switch", { name: "Follow" }));
    expect(onSubmit).not.toHaveBeenCalled();
  });
});

describe("Segmented", () => {
  afterEach(cleanup);

  function Harness() {
    const [v, setV] = useState<number | null>(30);
    return (
      <Segmented
        label="Range"
        value={v}
        onChange={setV}
        options={[
          { value: 7, label: "7d" },
          { value: 30, label: "30d" },
          { value: null, label: "All" },
        ]}
      />
    );
  }

  test("marks exactly the selected option as pressed and follows clicks, including a null value", () => {
    render(<Harness />);
    const pressed = () => screen.getAllByRole("button").filter((b) => b.getAttribute("aria-pressed") === "true").map((b) => b.textContent);
    expect(pressed()).toEqual(["30d"]);
    fireEvent.click(screen.getByRole("button", { name: "All" }));
    expect(pressed()).toEqual(["All"]);
    fireEvent.click(screen.getByRole("button", { name: "7d" }));
    expect(pressed()).toEqual(["7d"]);
  });
});

describe("Markdown images", () => {
  afterEach(cleanup);

  test("clicking an image opens a full-size view, dismissed by Escape", () => {
    render(<Markdown>{"![After](https://example.com/after.png)"}</Markdown>);
    expect(screen.queryByRole("dialog")).toBeNull();

    fireEvent.click(screen.getByRole("button", { name: "After" }));
    const dialog = screen.getByRole("dialog");
    expect(dialog).toBeTruthy();
    expect(within(dialog).getByAltText("After")).toBeTruthy();

    fireEvent.keyDown(window, { key: "Escape" });
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  test("Enter key on a focused image opens the full-size view", () => {
    render(<Markdown>{"![Before](https://example.com/before.png)"}</Markdown>);
    fireEvent.keyDown(screen.getByRole("button", { name: "Before" }), { key: "Enter" });
    expect(screen.getByRole("dialog")).toBeTruthy();
  });

  test("the full-size view links to the original image so oversized screenshots stay readable", () => {
    render(<Markdown>{"![After](https://example.com/after.png)"}</Markdown>);
    fireEvent.click(screen.getByRole("button", { name: "After" }));
    const dialog = screen.getByRole("dialog");
    const original = within(dialog).getByRole("link", { name: /open original/i }) as HTMLAnchorElement;
    expect(original.getAttribute("href")).toBe("https://example.com/after.png");
    expect(original.target).toBe("_blank");
  });

  test("opening the viewer moves focus into the dialog and closing restores it", () => {
    render(<Markdown>{"![After](https://example.com/after.png)"}</Markdown>);
    const trigger = screen.getByRole("button", { name: "After" });
    trigger.focus();
    expect(document.activeElement).toBe(trigger);

    fireEvent.click(trigger);
    const dialog = screen.getByRole("dialog");
    expect(dialog.contains(document.activeElement)).toBe(true);
    expect(document.activeElement).toBe(within(dialog).getByRole("button", { name: /close/i }));

    fireEvent.keyDown(window, { key: "Escape" });
    expect(document.activeElement).toBe(trigger);
  });

  test("Tab cycles focus between the dialog's controls without escaping it", () => {
    render(<Markdown>{"![After](https://example.com/after.png)"}</Markdown>);
    fireEvent.click(screen.getByRole("button", { name: "After" }));
    const dialog = screen.getByRole("dialog");
    const link = within(dialog).getByRole("link", { name: /open original/i });
    const closeBtn = within(dialog).getByRole("button", { name: /close/i });

    // The dialog only has two focusable controls; Tab past the last one
    // should wrap to the first, and Shift+Tab past the first back to the last.
    expect(document.activeElement).toBe(closeBtn);
    fireEvent.keyDown(closeBtn, { key: "Tab" });
    expect(document.activeElement).toBe(link);

    fireEvent.keyDown(link, { key: "Tab", shiftKey: true });
    expect(document.activeElement).toBe(closeBtn);
  });

  test("images inside a markdown link stay non-interactive so the link is the sole keyboard target", () => {
    render(<Markdown>{"[![Linked](https://example.com/linked.png)](https://example.com/target)"}</Markdown>);
    const link = screen.getByRole("link") as HTMLAnchorElement;
    expect(link.getAttribute("href")).toBe("https://example.com/target");

    const img = screen.getByAltText("Linked");
    expect(img.getAttribute("role")).toBeNull();
    expect(img.getAttribute("tabindex")).toBeNull();
    expect(img.onclick).toBeNull();
    expect(screen.queryByRole("dialog")).toBeNull();
  });
});
