import { useQueryClient } from "@tanstack/react-query";
import { useEffect, useState } from "react";

type DomainEvent = { type: string; project?: string | null; issue?: number | null; pr?: number | null; run?: number | null };

/** Subscribe to server events and invalidate the matching queries. */
export function useLiveEvents() {
  const qc = useQueryClient();
  const [connected, setConnected] = useState(false);
  useEffect(() => {
    let es: EventSource | null = null;
    let retry: ReturnType<typeof setTimeout> | undefined;
    const pending = new Set<string>();
    let flush: ReturnType<typeof setTimeout> | undefined;
    const invalidate = (key: string) => {
      pending.add(key);
      if (!flush) {
        flush = setTimeout(() => {
          for (const k of pending) qc.invalidateQueries({ queryKey: [k] });
          pending.clear();
          flush = undefined;
        }, 150);
      }
    };
    const connect = () => {
      es = new EventSource("/api/events/stream", { withCredentials: true });
      es.onopen = () => setConnected(true);
      es.onerror = () => {
        setConnected(false);
        es?.close();
        retry = setTimeout(connect, 3000);
      };
      es.addEventListener("resync", () => qc.invalidateQueries());
      es.addEventListener("domain", (e) => {
        const ev: DomainEvent = JSON.parse((e as MessageEvent).data);
        const t = ev.type;
        if (t.startsWith("issue") || t.startsWith("board") || t.startsWith("comment")) {
          invalidate("board");
          invalidate("issue");
          invalidate("inbox");
          invalidate("issues");
        }
        if (t.startsWith("pr") || t.startsWith("thread") || t.startsWith("comment")) {
          invalidate("pull");
          invalidate("board");
          invalidate("inbox");
        }
        if (t.startsWith("run")) {
          invalidate("runs");
          invalidate("run");
          invalidate("board");
          invalidate("issue");
        }
        if (t.startsWith("limits") || t.startsWith("agents")) {
          invalidate("agents");
          invalidate("limits");
          invalidate("board");
        }
        if (t.startsWith("settings")) {
          invalidate("settings");
          invalidate("board");
        }
        if (t.startsWith("projects") || t.startsWith("labels")) {
          invalidate("projects");
          invalidate("project");
          invalidate("board");
        }
        if (t.startsWith("permissions")) invalidate("permissions");
        if (t.startsWith("jobs")) invalidate("jobs");
      });
    };
    connect();
    return () => {
      es?.close();
      clearTimeout(retry);
      clearTimeout(flush);
    };
  }, [qc]);
  return connected;
}
