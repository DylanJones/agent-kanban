import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { useState } from "react";
import { afterEach, describe, expect, test, vi } from "vitest";
import { BrowserSettingsModal } from "./BrowserSettings";

afterEach(cleanup);

describe("BrowserSettingsModal", () => {
  test("marks the current theme as pressed and reports the other choices on click", () => {
    const onThemeChange = vi.fn();
    render(<BrowserSettingsModal open onClose={() => {}} theme="system" onThemeChange={onThemeChange} />);

    expect(screen.getByRole("button", { name: "System" }).getAttribute("aria-pressed")).toBe("true");
    expect(screen.getByRole("button", { name: "Light" }).getAttribute("aria-pressed")).toBe("false");
    expect(screen.getByRole("button", { name: "Dark" }).getAttribute("aria-pressed")).toBe("false");

    fireEvent.click(screen.getByRole("button", { name: "Dark" }));
    expect(onThemeChange).toHaveBeenCalledWith("dark");
  });

  test("renders nothing when closed", () => {
    render(<BrowserSettingsModal open={false} onClose={() => {}} theme="system" onThemeChange={() => {}} />);
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  test("closes on Escape", () => {
    const onClose = vi.fn();
    render(<BrowserSettingsModal open onClose={onClose} theme="system" onThemeChange={() => {}} />);
    fireEvent.keyDown(window, { key: "Escape" });
    expect(onClose).toHaveBeenCalled();
  });

  test("moves focus into the dialog and restores it to the trigger on close", () => {
    function Wrapper() {
      const [open, setOpen] = useState(false);
      return (
        <>
          <button onClick={() => setOpen(true)}>open settings</button>
          <BrowserSettingsModal open={open} onClose={() => setOpen(false)} theme="system" onThemeChange={() => {}} />
        </>
      );
    }
    render(<Wrapper />);
    const trigger = screen.getByRole("button", { name: "open settings" });
    trigger.focus();
    fireEvent.click(trigger);
    expect(screen.getByRole("dialog").contains(document.activeElement)).toBe(true);

    fireEvent.keyDown(window, { key: "Escape" });
    expect(document.activeElement).toBe(trigger);
  });
});
