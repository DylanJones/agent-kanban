import { ImagePlus } from "lucide-react";
import { type ChangeEvent, type Dispatch, type SetStateAction, useCallback, useEffect, useRef, useState } from "react";
import { ApiError } from "../api/client";
import { Spinner } from "./ui";

async function uploadImage(slug: string, file: File, signal: AbortSignal): Promise<{ markdown: string }> {
  const res = await fetch(`/api/projects/${slug}/attachments`, {
    method: "POST",
    credentials: "include",
    headers: { "Content-Type": file.type || "application/octet-stream" },
    body: file,
    signal,
  });
  const text = await res.text();
  const json = text ? JSON.parse(text) : undefined;
  if (!res.ok) throw new ApiError(res.status, json);
  return json;
}

/** Removes a pending-upload placeholder (and its trailing newline, if still present) from text. */
function stripPlaceholder(text: string, placeholder: string): string {
  const withNewline = `${placeholder}\n`;
  return text.includes(withNewline) ? text.replace(withNewline, "") : text.replace(placeholder, "");
}

/**
 * A markdown textarea that turns pasted/dropped/picked images into uploads: a placeholder is
 * inserted at the cursor immediately and swapped for `![](url)` once the upload finishes. Works
 * before the issue it belongs to exists — uploads only need a project.
 */
export function ImageTextarea({
  slug,
  value,
  onChange,
  onPendingChange,
  className,
  placeholder,
  autoFocus,
}: {
  slug: string;
  value: string;
  onChange: Dispatch<SetStateAction<string>>;
  /** Called with true while an upload is in flight, so the caller can block submission. */
  onPendingChange?: (pending: boolean) => void;
  className?: string;
  placeholder?: string;
  autoFocus?: boolean;
}) {
  const ref = useRef<HTMLTextAreaElement>(null);
  const fileInput = useRef<HTMLInputElement>(null);
  const [pending, setPending] = useState(0);
  const [error, setError] = useState<string | null>(null);
  const nextId = useRef(0);
  // Guards against uploads that finish after this editor instance unmounted (e.g. Cancel then
  // reopen Edit): the promise chain below must not touch a remounted instance's state.
  const alive = useRef(true);
  // In-flight uploads started by this instance, so unmount can abort them and scrub their
  // placeholders out of the parent's draft instead of leaving orphaned "Uploading…" text behind
  // (the parent keeps its draft across Cancel/reopen, it doesn't reset it).
  const inFlight = useRef<Map<number, { placeholder: string; controller: AbortController }>>(new Map());
  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
      for (const { placeholder, controller } of inFlight.current.values()) {
        controller.abort();
        onChange((prev) => stripPlaceholder(prev, placeholder));
      }
      inFlight.current.clear();
    };
  }, [onChange]);

  useEffect(() => onPendingChange?.(pending > 0), [pending, onPendingChange]);

  const upload = useCallback(
    (files: FileList | File[]) => {
      const imgs = Array.from(files).filter((f) => f.type.startsWith("image/"));
      if (imgs.length === 0) return;
      // Insert a unique placeholder at the cursor immediately, then replace that exact
      // placeholder text (wherever it ends up) once the upload resolves. Tracking the text
      // itself instead of a numeric offset keeps the insertion correct even if the draft is
      // edited, or other uploads insert their own markdown, before this one finishes.
      let at = ref.current?.selectionEnd ?? value.length;
      for (const file of imgs) {
        const id = nextId.current++;
        const placeholder = `![Uploading ${file.name}…](pending:${id})`;
        const insertText = `${placeholder}\n`;
        const insertAt = at;
        at += insertText.length;
        onChange((prev) => {
          const clamped = Math.min(insertAt, prev.length);
          return `${prev.slice(0, clamped)}${insertText}${prev.slice(clamped)}`;
        });
        setPending((p) => p + 1);
        setError(null);
        const controller = new AbortController();
        inFlight.current.set(id, { placeholder, controller });
        uploadImage(slug, file, controller.signal)
          .then(({ markdown }) => {
            if (!alive.current) return;
            onChange((prev) => (prev.includes(placeholder) ? prev.replace(placeholder, markdown) : prev));
          })
          .catch((e) => {
            if (!alive.current) return;
            setError(e instanceof Error ? e.message : String(e));
            onChange((prev) => stripPlaceholder(prev, placeholder));
          })
          .finally(() => {
            inFlight.current.delete(id);
            if (!alive.current) return;
            setPending((p) => p - 1);
          });
      }
    },
    [slug, onChange, value.length],
  );

  return (
    <div className="space-y-1">
      <div className="relative">
        <textarea
          ref={ref}
          autoFocus={autoFocus}
          className={className}
          placeholder={placeholder}
          value={value}
          onChange={(e) => onChange(e.target.value)}
          onPaste={(e) => {
            if (e.clipboardData.files.length > 0) upload(e.clipboardData.files);
          }}
          onDrop={(e) => {
            if (e.dataTransfer.files.length > 0) {
              e.preventDefault();
              upload(e.dataTransfer.files);
            }
          }}
          onDragOver={(e) => {
            if (e.dataTransfer.types.includes("Files")) e.preventDefault();
          }}
        />
        <button
          type="button"
          title="Attach a photo"
          onClick={() => fileInput.current?.click()}
          className="absolute bottom-1.5 right-1.5 rounded-md border border-zinc-300 dark:border-zinc-700 bg-white/90 dark:bg-zinc-900/90 p-1.5 text-zinc-500 hover:text-zinc-900 dark:hover:text-zinc-100"
        >
          <ImagePlus size={14} />
        </button>
        <input
          ref={fileInput}
          type="file"
          accept="image/*"
          multiple
          hidden
          onChange={(e: ChangeEvent<HTMLInputElement>) => {
            if (e.target.files?.length) upload(e.target.files);
            e.target.value = "";
          }}
        />
      </div>
      {pending > 0 && (
        <div className="flex items-center gap-1.5 text-xs text-zinc-500">
          <Spinner /> Uploading image{pending > 1 ? "s" : ""}…
        </div>
      )}
      {error && <div className="text-xs text-rose-600">{error}</div>}
    </div>
  );
}
