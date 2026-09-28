import { readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";

// Executes the actual inline pre-paint bootstrap script from index.html, so this test
// fails if the shipped script (not just theme.ts) regresses.
function runBootstrapScript() {
  const testFile = fileURLToPath(import.meta.url);
  const html = readFileSync(path.join(path.dirname(testFile), "index.html"), "utf8");
  const match = html.match(/<script>([\s\S]*?)<\/script>/);
  if (!match) throw new Error("could not find inline bootstrap script in index.html");
  new Function(match[1])();
}

function mockMatchMedia(matches: boolean) {
  vi.stubGlobal(
    "matchMedia",
    vi.fn().mockReturnValue({ matches, media: "(prefers-color-scheme: dark)" }),
  );
}

beforeEach(() => {
  localStorage.clear();
  document.documentElement.classList.remove("dark");
  document.documentElement.style.colorScheme = "";
});

afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe("index.html inline theme bootstrap", () => {
  test("applies dark before paint when the OS prefers dark and nothing is stored", () => {
    mockMatchMedia(true);
    runBootstrapScript();
    expect(document.documentElement.classList.contains("dark")).toBe(true);
    expect(document.documentElement.style.colorScheme).toBe("dark");
  });

  test("applies light before paint when the OS prefers light and nothing is stored", () => {
    mockMatchMedia(false);
    runBootstrapScript();
    expect(document.documentElement.classList.contains("dark")).toBe(false);
    expect(document.documentElement.style.colorScheme).toBe("light");
  });

  test("falls back to system and still applies before paint when localStorage throws", () => {
    mockMatchMedia(true);
    vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => {
      throw new Error("blocked");
    });
    runBootstrapScript();
    expect(document.documentElement.classList.contains("dark")).toBe(true);
    expect(document.documentElement.style.colorScheme).toBe("dark");
  });
});
