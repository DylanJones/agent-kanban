import { Box, Info, Terminal } from "lucide-react";
import { useId, useState } from "react";
import { fmtTokens } from "../pages/Usage";

/** What a run's (or the Usage page's) token total means. */
export const TOTAL_TOKENS_HELP =
  "Total across every request in the run: each request's input (including cache reads) plus its output, summed. Agents resend the conversation on every turn, so this grows well past the context window and isn't what's loaded at any one time.";

/** What the adapter-reported context figure means. */
export const CONTEXT_HELP =
  "What the agent reports is in its context window right now (system prompt, tools, conversation so far) out of the window's capacity.";

/** "context 46k / 258k (18%)" from an adapter usage report, or null when it has none. */
export function contextLabel(used: unknown, size: unknown): string | null {
  const u = Number(used);
  if (used == null || !Number.isFinite(u)) return null;
  const s = Number(size);
  if (size == null || !Number.isFinite(s) || s <= 0) return `context ${fmtTokens(u)}`;
  return `context ${fmtTokens(u)} / ${fmtTokens(s)} (${Math.round((u / s) * 100)}%)`;
}

const chip = "inline-flex min-w-0 items-center gap-1.5 rounded-md bg-surface-3/70 px-2 py-0.5 text-[11px] text-fg-muted";

/**
 * Run-detail chips for total tokens and current context, plus a button that
 * reveals what each figure means (hover titles alone don't work on phones).
 */
export function RunUsageChips({ totalTokens, models, costUsd, context }: { totalTokens: number; models: string[]; costUsd?: number | null; context?: { used?: unknown; size?: unknown } }) {
  const [open, setOpen] = useState(false);
  const helpId = useId();
  const ctx = context ? contextLabel(context.used, context.size) : null;
  if (totalTokens <= 0 && !ctx) return null;
  return (
    <>
      {totalTokens > 0 && (
        <span title={`${TOTAL_TOKENS_HELP}${models.length ? `\n\nModels: ${models.join(", ")}` : ""}`} className={chip}>
          <Terminal size={12} className="shrink-0 text-fg-subtle" />
          <span className="truncate">
            {fmtTokens(totalTokens)} tokens total{costUsd != null ? ` · $${costUsd.toFixed(2)}*` : ""}
          </span>
        </span>
      )}
      {ctx && (
        <span title={CONTEXT_HELP} className={chip}>
          <Box size={12} className="shrink-0 text-fg-subtle" />
          <span className="truncate">{ctx}</span>
        </span>
      )}
      <button
        type="button"
        onClick={() => setOpen((o) => !o)}
        aria-expanded={open}
        aria-controls={helpId}
        aria-label="What do these token numbers mean?"
        className="inline-flex size-6 items-center justify-center rounded-md text-fg-subtle hover:bg-surface-3 hover:text-fg-muted"
      >
        <Info size={13} />
      </button>
      {open && (
        <div id={helpId} className="order-last basis-full space-y-1 rounded-md bg-surface-3/50 px-2.5 py-2 text-[11px] leading-relaxed text-fg-muted">
          {totalTokens > 0 && (
            <p>
              <span className="font-medium text-fg">Tokens total</span> — {TOTAL_TOKENS_HELP}
              {models.length > 0 && ` Models: ${models.join(", ")}.`}
            </p>
          )}
          {ctx && (
            <p>
              <span className="font-medium text-fg">Context</span> — {CONTEXT_HELP}
            </p>
          )}
        </div>
      )}
    </>
  );
}
