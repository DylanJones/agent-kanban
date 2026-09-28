#!/usr/bin/env python3
"""A scripted ACP agent for tests. Speaks newline-delimited JSON-RPC on stdio.

Behaviour is chosen by FAKE_MODE and uses the board API via $AKB_API / $AKB_AUTH:
  triage          move the issue to `ready`
  fix             commit a change, open a PR if none, reply to open threads, move to in_review
  fix_and_bug     like fix, but first file an unrelated bug with text/plain
  review_approve  approve the current PR head
  review_changes  add a blocking thread and request changes
  merge_prep      merge the base branch (taking ours on conflict), commit, move to in_review
  decide          request a human decision
  noop            reply with text only (never records an outcome)
  limit           fail the prompt with a Claude-style usage-limit error
  hang            never finish the turn (until cancelled)
"""
import json
import os
import subprocess
import sys
import threading
import time
import urllib.request

API = os.environ.get("AKB_API", "")
AUTH = os.environ.get("AKB_AUTH", "")
MODE = os.environ.get("FAKE_MODE", "noop")
cancelled = threading.Event()
out_lock = threading.Lock()
pending = {}  # request id -> [Event, response]
# Session settings, shaped like the real adapters' ACP configOptions.
CONFIG = [
    {"id": "mode", "name": "Mode", "category": "mode", "type": "select", "currentValue": "default", "options": [{"value": "default", "name": "Default"}]},
    {"id": "model", "name": "Model", "category": "model", "type": "select", "currentValue": "small", "options": [{"value": "small", "name": "Small"}, {"value": "big", "name": "Big"}]},
    {"id": "effort", "name": "Effort", "category": "thought_level", "type": "select", "currentValue": "low", "options": [{"value": "low", "name": "Low"}, {"value": "high", "name": "High"}]},
]


def send(msg):
    with out_lock:
        sys.stdout.write(json.dumps(msg) + "\n")
        sys.stdout.flush()


def say(sid, text):
    send({"jsonrpc": "2.0", "method": "session/update", "params": {"sessionId": sid, "update": {"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": text}}}})


def tool(sid, tid, title, status):
    kind = "tool_call" if status == "pending" else "tool_call_update"
    send({"jsonrpc": "2.0", "method": "session/update", "params": {"sessionId": sid, "update": {"sessionUpdate": kind, "toolCallId": tid, "title": title, "status": status}}})


def http(method, path, body=None, text=None):
    data, headers = None, {"Authorization": f"Bearer {AUTH}"}
    if text is not None:
        data, headers["Content-Type"] = text.encode(), "text/plain"
    elif body is not None:
        data, headers["Content-Type"] = json.dumps(body).encode(), "application/json"
    req = urllib.request.Request(API + path, data=data, headers=headers, method=method)
    try:
        with urllib.request.urlopen(req) as r:
            raw = r.read()
            return json.loads(raw) if raw else None
    except urllib.error.HTTPError as e:
        return {"error": e.code, "body": e.read().decode()}


def ask_permission(sid, rid, kind, command):
    ev = threading.Event()
    pending[rid] = [ev, None]
    send({"jsonrpc": "2.0", "id": rid, "method": "session/request_permission", "params": {
        "sessionId": sid,
        "toolCall": {"toolCallId": rid, "title": command, "kind": kind, "rawInput": {"command": command}},
        "options": [
            {"optionId": "allow", "name": "Allow", "kind": "allow_once"},
            {"optionId": "reject", "name": "Reject", "kind": "reject_once"},
        ]}})
    ev.wait(30)
    res = pending.pop(rid)[1] or {}
    outcome = res.get("result", {}).get("outcome", {})
    return outcome.get("optionId") or outcome.get("outcome")


def git(*args):
    return subprocess.run(["git", *args], capture_output=True, text=True, check=False).stdout.strip()


def behave(sid):
    cur = http("GET", "/runs/current")
    p, n = cur["project"], cur["issue"]
    if MODE == "triage":
        http("POST", f"/projects/{p}/issues/{n}/comments", {"body": "Reproduced; root cause in main.txt."})
        http("POST", f"/projects/{p}/issues/{n}/transition", {"to": "ready"})
    elif MODE in ("fix", "fix_and_bug"):
        if MODE == "fix_and_bug":
            r = http("POST", f"/projects/{p}/issues", text="Side bug found while fixing\nSomething else is broken.")
            say(sid, f"filed #{r.get('number')}\n")
        tool(sid, "t1", "edit fix.txt", "pending")
        with open("fix.txt", "a") as f:
            f.write(f"fix at {time.time()}\n")
        git("add", "-A")
        git("-c", "user.name=Fake", "-c", "user.email=fake@example.com", "commit", "-q", "-m", "🐛 Fix the thing")
        tool(sid, "t1", "edit fix.txt", "completed")
        if cur.get("pr") is None:
            http("POST", f"/projects/{p}/pulls", {"title": "🐛 Fix the thing", "body": "Tests: all pass"})
        else:
            for t in http("GET", f"/projects/{p}/pulls/{cur['pr']}/threads?resolved=false"):
                http("POST", f"/threads/{t['id']}/replies", {"body": "Fixed."})
        r = http("POST", f"/projects/{p}/issues/{n}/transition", {"to": "in_review"})
        say(sid, f"moved: {json.dumps(r)[:200]}\n")
    elif MODE in ("review_approve", "review_changes"):
        pr = http("GET", f"/projects/{p}/pulls/{cur['pr']}")
        head = pr["head_sha"]
        if MODE == "review_changes":
            diff = http("GET", f"/projects/{p}/pulls/{cur['pr']}/diff")
            f = diff["files"][0]["path"]
            http("POST", f"/projects/{p}/pulls/{cur['pr']}/threads", {"path": f, "line": 1, "body": "Please handle the empty case.", "severity": "blocking"})
            http("POST", f"/projects/{p}/pulls/{cur['pr']}/reviews", {"verdict": "changes_requested", "commit_sha": head, "body": "One blocking issue."})
        else:
            for t in http("GET", f"/projects/{p}/pulls/{cur['pr']}/threads?resolved=false"):
                http("POST", f"/threads/{t['id']}/resolve", {"comment": "Verified."})
            r = http("POST", f"/projects/{p}/pulls/{cur['pr']}/reviews", {"verdict": "approve", "commit_sha": head, "body": "LGTM"})
            say(sid, f"verdict: {json.dumps(r)[:200]}\n")
    elif MODE == "merge_prep":
        base = os.environ.get("FAKE_BASE", "master")
        git("-c", "user.name=Fake", "-c", "user.email=fake@example.com", "merge", "-q", "-X", "ours", base, "-m", "🔀 Merge base")
        http("POST", f"/projects/{p}/issues/{n}/transition", {"to": "in_review"})
    elif MODE == "decide":
        http("POST", f"/projects/{p}/issues/{n}/decision-request", {"question": "Option A or B?", "options": ["A", "B"], "consequences": "A is simpler."})
    elif MODE == "perm":
        results = {c: ask_permission(sid, f"perm-{i}", "execute", c) for i, c in enumerate(["ninja -C build tests", "git push origin HEAD"])}
        http("POST", f"/projects/{p}/issues/{n}/comments", {"body": json.dumps(results)})
        http("POST", f"/projects/{p}/issues/{n}/transition", {"to": "ready"})
    elif MODE == "noop":
        say(sid, "I looked around but did nothing.")


def handle_prompt(msg):
    sid = msg["params"]["sessionId"]
    text = "".join(b.get("text", "") for b in msg["params"]["prompt"])
    if MODE == "limit":
        say(sid, "You've hit your limit · resets 5pm (America/Los_Angeles)")
        send({"jsonrpc": "2.0", "id": msg["id"], "error": {"code": -32603, "message": "You've hit your limit · resets 5pm (America/Los_Angeles)", "data": {"errorKind": "rate_limit"}}})
        return
    if MODE == "hang":
        while not cancelled.is_set():
            time.sleep(0.05)
        send({"jsonrpc": "2.0", "id": msg["id"], "result": {"stopReason": "cancelled"}})
        return
    if "without recording an outcome" in text:
        say(sid, "Still nothing to do.")
    else:
        try:
            behave(sid)
        except Exception as e:  # report and end the turn
            say(sid, f"error: {e}")
    send({"jsonrpc": "2.0", "id": msg["id"], "result": {"stopReason": "end_turn"}})


def main():
    for line in sys.stdin:
        if not line.strip():
            continue
        msg = json.loads(line)
        m = msg.get("method")
        if m == "initialize":
            send({"jsonrpc": "2.0", "id": msg["id"], "result": {"protocolVersion": 1, "agentCapabilities": {}, "authMethods": [], "agentInfo": {"name": "fake", "version": "0"}}})
        elif m == "session/new":
            send({"jsonrpc": "2.0", "id": msg["id"], "result": {"sessionId": "s1", "modes": {"currentModeId": "default", "availableModes": [{"id": "default", "name": "Default"}]}, "configOptions": CONFIG}})
        elif m == "session/set_config_option":
            for o in CONFIG:
                if o["id"] == msg["params"]["configId"]:
                    o["currentValue"] = msg["params"]["value"]
            send({"jsonrpc": "2.0", "id": msg["id"], "result": {"configOptions": CONFIG}})
        elif m == "session/set_mode":
            send({"jsonrpc": "2.0", "id": msg["id"], "result": {}})
        elif m == "session/prompt":
            threading.Thread(target=handle_prompt, args=(msg,), daemon=True).start()
        elif m == "session/cancel":
            cancelled.set()
        elif m is None and msg.get("id") in pending:
            pending[msg["id"]][1] = msg
            pending[msg["id"]][0].set()
        elif "id" in msg and m is not None:
            send({"jsonrpc": "2.0", "id": msg["id"], "error": {"code": -32601, "message": "method not found"}})


if __name__ == "__main__":
    main()
