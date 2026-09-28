import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, test } from "vitest";
import { Markdown, fieldCls, inputCls } from "./ui";

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
