import { act, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";
import { THEME_STORAGE_KEY, applyTheme, effectiveTheme, readStoredThemePreference, useThemePreference } from "./theme";

function mockMatchMedia(matches: boolean) {
  const listeners = new Set<(e: { matches: boolean }) => void>();
  const mql = {
    matches,
    media: "(prefers-color-scheme: dark)",
    addEventListener: (_: string, cb: (e: { matches: boolean }) => void) => listeners.add(cb),
    removeEventListener: (_: string, cb: (e: { matches: boolean }) => void) => listeners.delete(cb),
  };
  vi.stubGlobal("matchMedia", vi.fn().mockReturnValue(mql));
  return {
    fire(next: boolean) {
      mql.matches = next;
      listeners.forEach((cb) => cb({ matches: next }));
    },
  };
}

beforeEach(() => {
  localStorage.clear();
  document.documentElement.classList.remove("dark");
  document.documentElement.style.colorScheme = "";
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("readStoredThemePreference", () => {
  test("defaults to system when nothing is stored", () => {
    expect(readStoredThemePreference()).toBe("system");
  });

  test("reads a valid stored preference", () => {
    localStorage.setItem(THEME_STORAGE_KEY, "dark");
    expect(readStoredThemePreference()).toBe("dark");
  });

  test("falls back to system for a corrupt stored value", () => {
    localStorage.setItem(THEME_STORAGE_KEY, "purple");
    expect(readStoredThemePreference()).toBe("system");
  });

  test("falls back to system when localStorage throws", () => {
    const spy = vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => {
      throw new Error("blocked");
    });
    expect(readStoredThemePreference()).toBe("system");
    spy.mockRestore();
  });
});

describe("effectiveTheme / applyTheme", () => {
  test("system resolves to the OS preference", () => {
    mockMatchMedia(true);
    expect(effectiveTheme("system")).toBe("dark");
    mockMatchMedia(false);
    expect(effectiveTheme("system")).toBe("light");
  });

  test("explicit light stays light even when the OS prefers dark", () => {
    mockMatchMedia(true);
    expect(effectiveTheme("light")).toBe("light");
  });

  test("explicit dark stays dark even when the OS prefers light", () => {
    mockMatchMedia(false);
    expect(effectiveTheme("dark")).toBe("dark");
  });

  test("applyTheme sets the dark class and color-scheme together", () => {
    mockMatchMedia(false);
    applyTheme("dark");
    expect(document.documentElement.classList.contains("dark")).toBe(true);
    expect(document.documentElement.style.colorScheme).toBe("dark");

    applyTheme("light");
    expect(document.documentElement.classList.contains("dark")).toBe(false);
    expect(document.documentElement.style.colorScheme).toBe("light");
  });
});

describe("useThemePreference", () => {
  test("persists an explicit choice and applies it immediately", () => {
    mockMatchMedia(false);
    const { result } = renderHook(() => useThemePreference());
    act(() => result.current[1]("dark"));
    expect(result.current[0]).toBe("dark");
    expect(localStorage.getItem(THEME_STORAGE_KEY)).toBe("dark");
    expect(document.documentElement.classList.contains("dark")).toBe(true);
  });

  test("reacts to OS changes only while in system mode", () => {
    const media = mockMatchMedia(false);
    const { result } = renderHook(() => useThemePreference());
    act(() => result.current[1]("system"));
    expect(document.documentElement.classList.contains("dark")).toBe(false);

    act(() => media.fire(true));
    expect(document.documentElement.classList.contains("dark")).toBe(true);

    act(() => result.current[1]("light"));
    act(() => media.fire(true));
    // Explicit Light must not be pulled back to dark by a subsequent OS change.
    expect(document.documentElement.classList.contains("dark")).toBe(false);
  });
});
