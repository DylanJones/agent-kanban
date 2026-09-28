import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import clsx from "clsx";
import { Check, Copy, KeyRound, Loader2, ShieldCheck } from "lucide-react";
import { useState } from "react";
import { useSearchParams } from "react-router";
import { ApiError, type S, api, client, unwrap } from "../api/client";
import { Button, ErrorBox, Modal, inputCls } from "./ui";

const COMMAND = "npx @anthropic-ai/claude-code setup-token";

/** Opens the "Connect Claude" flow from anywhere via `?connect=claude`. */
export function useConnectClaude() {
  const [sp, setSp] = useSearchParams();
  const open = () => {
    const next = new URLSearchParams(sp);
    next.set("connect", "claude");
    setSp(next);
  };
  return open;
}

export function useCredentials() {
  return useQuery({ queryKey: ["agents", "credentials"], queryFn: () => unwrap(client.GET("/api/credentials")) });
}

function Step({ n, title, done, children }: { n: number; title: string; done?: boolean; children: React.ReactNode }) {
  return (
    <div className="flex gap-3">
      <div
        className={clsx(
          "mt-0.5 flex h-6 w-6 shrink-0 items-center justify-center rounded-full text-xs font-semibold",
          done ? "bg-emerald-600 text-white" : "bg-zinc-200 text-zinc-700 dark:bg-zinc-800 dark:text-zinc-300",
        )}
      >
        {done ? <Check size={13} /> : n}
      </div>
      <div className="min-w-0 flex-1 space-y-2">
        <div className="text-sm font-medium">{title}</div>
        {children}
      </div>
    </div>
  );
}

export function ConnectClaudeModal() {
  const [sp, setSp] = useSearchParams();
  const qc = useQueryClient();
  const creds = useCredentials();
  const [token, setToken] = useState("");
  const [copied, setCopied] = useState(false);
  const [result, setResult] = useState<S["SavedCredential"] | null>(null);
  const open = sp.get("connect") === "claude";
  const close = () => {
    const next = new URLSearchParams(sp);
    next.delete("connect");
    setSp(next);
    setToken("");
    setResult(null);
    save.reset();
  };
  const save = useMutation({
    mutationFn: async () => {
      try {
        return await api<S["SavedCredential"]>("PUT", "/api/credentials/claude-token", { token, verify: true });
      } catch (e) {
        // 422 carries the failed verification run.
        if (e instanceof ApiError && e.status === 422) return e.problem as unknown as S["SavedCredential"];
        throw e;
      }
    },
    onSuccess: (r) => {
      setResult(r);
      if (r.saved) {
        setToken("");
        qc.invalidateQueries();
      }
    },
  });
  if (!open) return null;
  const containers = creds.data?.container_projects ?? [];
  const failed = result && !result.saved;
  return (
    <Modal open onClose={close} title={<span className="flex items-center gap-2"><KeyRound size={16} /> Connect Claude for containers</span>} wide>
      {result?.saved ? (
        <div className="space-y-3">
          <div className="flex items-start gap-2 rounded-md border border-emerald-300 bg-emerald-50 p-3 text-sm dark:border-emerald-900 dark:bg-emerald-950/30">
            <ShieldCheck size={16} className="mt-0.5 text-emerald-600" />
            <div>
              Claude is connected.{" "}
              {result.tested_in ? (
                <>It answered from inside the <b>{result.tested_in}</b> container, so queued Claude work will start on the next scheduler pass.</>
              ) : (
                <>It'll be used the next time a Claude run starts in a container.</>
              )}
            </div>
          </div>
          <div className="flex justify-end">
            <Button variant="primary" onClick={close}>
              Done
            </Button>
          </div>
        </div>
      ) : (
        <div className="space-y-5">
          <p className="text-sm text-zinc-600 dark:text-zinc-400">
            {containers.length ? (
              <>
                <b>{containers.join(", ")}</b> {containers.length === 1 ? "runs" : "run"} agents in Docker containers. Claude normally signs in with the login stored in your Mac's Keychain, which a
                container can't reach, so it needs a long-lived token from your Claude subscription instead. You only do this once.
              </>
            ) : (
              <>Agents running in Docker containers can't use the Claude login in your Mac's Keychain, so Claude needs a long-lived token from your subscription. You only do this once.</>
            )}
          </p>
          <Step n={1} title="Create a token in a terminal">
            <div className="flex items-center gap-2">
              <code className="flex-1 rounded-md bg-zinc-100 px-3 py-2 font-mono text-xs dark:bg-zinc-800">{COMMAND}</code>
              <Button
                size="sm"
                onClick={() => {
                  navigator.clipboard?.writeText(COMMAND);
                  setCopied(true);
                  setTimeout(() => setCopied(false), 1500);
                }}
              >
                {copied ? <Check size={12} /> : <Copy size={12} />} {copied ? "Copied" : "Copy"}
              </Button>
            </div>
            <p className="text-xs text-zinc-500">It opens your browser to approve access with your Claude account, then prints a token starting with <code>sk-ant-oat</code>.</p>
          </Step>
          <Step n={2} title="Paste it here">
            <input
              type="password"
              autoComplete="off"
              spellCheck={false}
              className={clsx(inputCls, "font-mono")}
              placeholder="sk-ant-oat…"
              value={token}
              onChange={(e) => setToken(e.target.value)}
            />
            <p className="text-xs text-zinc-500">
              It's stored in <code>~/.agent-kanban/secrets.toml</code> (readable only by you) and passed only to Claude containers. This page never shows it again.
            </p>
          </Step>
          <Step n={3} title="Check it works">
            <p className="text-xs text-zinc-500">
              {containers.length
                ? "Saving starts Claude inside the project's container and sends it a one-line prompt (a few tokens). The token is saved only if that works."
                : "No project uses containers yet, so the token is saved without a test run."}
            </p>
            {failed && (
              <div className="space-y-1 rounded-md border border-rose-300 bg-rose-50 p-3 text-xs dark:border-rose-900 dark:bg-rose-950/30">
                <div className="font-medium text-rose-800 dark:text-rose-300">
                  Claude couldn't sign in with that token{result.test?.limit === "auth" ? "" : result.test?.limit ? ` (it hit a ${result.test.limit} limit)` : ""}. Nothing was saved.
                </div>
                {result.test?.error && <div className="text-rose-700 dark:text-rose-400">{result.test.error}</div>}
                <div className="text-zinc-600 dark:text-zinc-400">Check you copied the whole token, or run the command again for a new one.</div>
              </div>
            )}
            <ErrorBox error={save.error} />
            <div className="flex items-center justify-end gap-2">
              <Button variant="ghost" onClick={close}>
                Cancel
              </Button>
              <Button variant="primary" disabled={!token.trim() || save.isPending} onClick={() => save.mutate()}>
                {save.isPending ? (
                  <>
                    <Loader2 size={13} className="animate-spin" /> Testing in container…
                  </>
                ) : (
                  "Test and save"
                )}
              </Button>
            </div>
          </Step>
        </div>
      )}
    </Modal>
  );
}
