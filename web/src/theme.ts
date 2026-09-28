import { useCallback, useEffect, useState } from "react";

export type ThemePreference = "system" | "light" | "dark";
export type EffectiveTheme = "light" | "dark";

export const THEME_STORAGE_KEY = "akb-theme";

function systemPrefersDark(): boolean {
  return typeof matchMedia === "function" && matchMedia("(prefers-color-scheme: dark)").matches;
}

export function readStoredThemePreference(): ThemePreference {
  try {
    const v = localStorage.getItem(THEME_STORAGE_KEY);
    if (v === "light" || v === "dark" || v === "system") return v;
  } catch {
    // localStorage unavailable (private browsing, disabled, quota) — fall back to system.
  }
  return "system";
}

export function effectiveTheme(pref: ThemePreference): EffectiveTheme {
  return pref === "system" ? (systemPrefersDark() ? "dark" : "light") : pref;
}

/** Applies the effective theme to the document. Tailwind's `dark:` utilities key off the `dark`
 * class (see the `@custom-variant dark` override in index.css), so this is the single place that
 * decides light vs. dark for the whole app, including the diff and usage chart colors. */
export function applyTheme(pref: ThemePreference) {
  const effective = effectiveTheme(pref);
  const root = document.documentElement;
  root.classList.toggle("dark", effective === "dark");
  root.style.colorScheme = effective;
}

export function useThemePreference(): [ThemePreference, (pref: ThemePreference) => void] {
  const [pref, setPref] = useState<ThemePreference>(readStoredThemePreference);

  useEffect(() => {
    applyTheme(pref);
  }, [pref]);

  // Only System should track the OS live; an explicit Light/Dark choice must not shift underneath it.
  useEffect(() => {
    if (pref !== "system" || typeof matchMedia !== "function") return;
    const mql = matchMedia("(prefers-color-scheme: dark)");
    const onChange = () => applyTheme("system");
    mql.addEventListener("change", onChange);
    return () => mql.removeEventListener("change", onChange);
  }, [pref]);

  const setPreference = useCallback((next: ThemePreference) => {
    try {
      localStorage.setItem(THEME_STORAGE_KEY, next);
    } catch {
      // Theme still applies for this page load; it just won't persist across reloads.
    }
    setPref(next);
  }, []);

  return [pref, setPreference];
}
