import { act, cleanup, render } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";
import { ImageTextarea } from "./ImageTextarea";

function file(name = "shot.png") {
  return new File(["x"], name, { type: "image/png" });
}

function deferred<T>() {
  let resolve!: (v: T) => void;
  let reject!: (e: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

function textareaEl(container: HTMLElement) {
  return container.querySelector("textarea") as HTMLTextAreaElement;
}

function pasteFiles(el: HTMLTextAreaElement, files: File[]) {
  const clipboardData = { files, types: ["Files"] };
  const event = new Event("paste", { bubbles: true, cancelable: true }) as unknown as ClipboardEvent;
  Object.defineProperty(event, "clipboardData", { value: clipboardData });
  act(() => {
    el.dispatchEvent(event);
  });
}

describe("ImageTextarea", () => {
  let fetchMock: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    fetchMock = vi.fn();
    vi.stubGlobal("fetch", fetchMock);
  });

  afterEach(() => {
    cleanup();
    vi.unstubAllGlobals();
  });

  test("inserts the upload at its anchor even if the draft changes first", async () => {
    const d = deferred<Response>();
    fetchMock.mockReturnValueOnce(d.promise);

    let value = "hello world";
    const onChange = vi.fn((next: string | ((prev: string) => string)) => {
      value = typeof next === "function" ? next(value) : next;
    });

    const { container, rerender } = render(<ImageTextarea slug="proj" value={value} onChange={onChange} />);
    const el = textareaEl(container);
    el.setSelectionRange(6, 6); // cursor between "hello " and "world"

    pasteFiles(el, [file()]);
    rerender(<ImageTextarea slug="proj" value={value} onChange={onChange} />);

    // The user edits the draft elsewhere before the upload resolves.
    value = `PREFIX ${value}`;
    rerender(<ImageTextarea slug="proj" value={value} onChange={onChange} />);

    await act(async () => {
      d.resolve({ ok: true, text: async () => JSON.stringify({ markdown: "![](/image.png)" }) } as Response);
      await d.promise;
    });

    expect(value).toBe("PREFIX hello ![](/image.png)\nworld");
  });

  test("two overlapping uploads resolving out of order don't corrupt each other", async () => {
    const first = deferred<Response>();
    const second = deferred<Response>();
    fetchMock.mockReturnValueOnce(first.promise).mockReturnValueOnce(second.promise);

    let value = "AB";
    const onChange = vi.fn((next: string | ((prev: string) => string)) => {
      value = typeof next === "function" ? next(value) : next;
    });

    const { container, rerender } = render(<ImageTextarea slug="proj" value={value} onChange={onChange} />);
    const el = textareaEl(container);

    el.setSelectionRange(2, 2); // at the end, after "AB"
    pasteFiles(el, [file("one.png")]);
    rerender(<ImageTextarea slug="proj" value={value} onChange={onChange} />);

    el.setSelectionRange(0, 0); // back at the start, before "AB"
    pasteFiles(el, [file("two.png")]);
    rerender(<ImageTextarea slug="proj" value={value} onChange={onChange} />);

    // The upload inserted at the start resolves first...
    await act(async () => {
      second.resolve({ ok: true, text: async () => JSON.stringify({ markdown: "![](/two.png)" }) } as Response);
      await second.promise;
    });
    // ...then the one inserted at the end, whose anchor sits after the text "two" just inserted.
    await act(async () => {
      first.resolve({ ok: true, text: async () => JSON.stringify({ markdown: "![](/one.png)" }) } as Response);
      await first.promise;
    });

    expect(value).toBe("![](/two.png)\nAB![](/one.png)\n");
  });

  test("a failed upload removes only its own placeholder", async () => {
    const d = deferred<Response>();
    fetchMock.mockReturnValueOnce(d.promise);

    let value = "hello";
    const onChange = vi.fn((next: string | ((prev: string) => string)) => {
      value = typeof next === "function" ? next(value) : next;
    });

    const { container, rerender } = render(<ImageTextarea slug="proj" value={value} onChange={onChange} />);
    const el = textareaEl(container);
    el.setSelectionRange(5, 5);
    pasteFiles(el, [file()]);
    rerender(<ImageTextarea slug="proj" value={value} onChange={onChange} />);
    expect(value).toContain("pending:0");

    await act(async () => {
      d.reject(new Error("boom"));
      await d.promise.catch(() => {});
    });

    expect(value).toBe("hello");
  });

  test("an upload that resolves after unmount does not touch a remounted editor's state", async () => {
    const d = deferred<Response>();
    fetchMock.mockReturnValueOnce(d.promise);

    let value = "draft";
    const onChange = vi.fn((next: string | ((prev: string) => string)) => {
      value = typeof next === "function" ? next(value) : next;
    });

    const { container, rerender, unmount } = render(<ImageTextarea slug="proj" value={value} onChange={onChange} />);
    const el = textareaEl(container);
    el.setSelectionRange(value.length, value.length);
    pasteFiles(el, [file()]);
    rerender(<ImageTextarea slug="proj" value={value} onChange={onChange} />);

    // Simulate Cancel: the editor unmounts while the upload is still in flight. The parent (as
    // IssueBody and NewIssueModal both do) keeps whatever draft it last had — including the
    // pending placeholder — rather than resetting it, so `value` is NOT reassigned here.
    unmount();

    // The unmount must have scrubbed the orphaned placeholder out of the retained draft itself,
    // not just guarded against the stale promise touching it later.
    expect(value).toBe("draft");

    // Reopen Edit: a fresh editor instance mounts over that (now clean) retained value.
    render(<ImageTextarea slug="proj" value={value} onChange={onChange} />);

    await act(async () => {
      d.resolve({ ok: true, text: async () => JSON.stringify({ markdown: "![](/late.png)" }) } as Response);
      await d.promise;
    });

    // The stale upload must not have mutated the reopened draft.
    expect(value).toBe("draft");
  });

  test("cancel then reopen does not let a new upload collide with the cleaned-up placeholder id", async () => {
    const first = deferred<Response>();
    const second = deferred<Response>();
    fetchMock.mockReturnValueOnce(first.promise).mockReturnValueOnce(second.promise);

    let value = "draft";
    const onChange = vi.fn((next: string | ((prev: string) => string)) => {
      value = typeof next === "function" ? next(value) : next;
    });

    const { container, rerender, unmount } = render(<ImageTextarea slug="proj" value={value} onChange={onChange} />);
    const el = textareaEl(container);
    el.setSelectionRange(value.length, value.length);
    pasteFiles(el, [file("shot.png")]); // gets id pending:0 in this (soon cancelled) instance
    rerender(<ImageTextarea slug="proj" value={value} onChange={onChange} />);
    expect(value).toBe("draft![Uploading shot.png…](pending:0)\n");

    unmount(); // Cancel: orphaned placeholder is scrubbed immediately
    expect(value).toBe("draft");

    // Reopen Edit and start a new upload of the same filename; the fresh instance also starts
    // counting from pending:0.
    const { container: container2 } = render(<ImageTextarea slug="proj" value={value} onChange={onChange} />);
    const el2 = textareaEl(container2);
    el2.setSelectionRange(value.length, value.length);
    pasteFiles(el2, [file("shot.png")]);
    expect(value).toBe("draft![Uploading shot.png…](pending:0)\n");

    // The cancelled instance's stale upload resolving must not clobber the new one's placeholder...
    await act(async () => {
      first.resolve({ ok: true, text: async () => JSON.stringify({ markdown: "![](/stale.png)" }) } as Response);
      await first.promise;
    });
    expect(value).toBe("draft![Uploading shot.png…](pending:0)\n");

    // ...and the live upload still resolves into its own placeholder correctly.
    await act(async () => {
      second.resolve({ ok: true, text: async () => JSON.stringify({ markdown: "![](/live.png)" }) } as Response);
      await second.promise;
    });
    expect(value).toBe("draft![](/live.png)\n");
  });
});
