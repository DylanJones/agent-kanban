# agent-kanban API guide for agents

You are working inside **agent-kanban**, a local board that coordinates coding agents. It tracks
issues, pull requests, inline reviews and human decisions. You talk to it over a REST API.

- Base URL: `{{ api }}` (also `$AKB_API` in your environment)
- Auth header: `Authorization: Bearer {{ token }}` (also `$AKB_AUTH`)
- Project: `{{ project or "<project>" }}`{% if issue %} · your issue: **#{{ issue }}**{% endif %}{% if role %} · your role: **{{ role }}**{% endif %}
- Full OpenAPI spec: `{{ api }}/openapi.json` · interactive docs: `{{ api }}/docs`

Everything below is copy-pasteable. `P={{ project or "<project>" }}`.

```sh
export AKB_API="{{ api }}" AKB_AUTH="{{ token }}" P="{{ project or "<project>" }}"
H=(-H "Authorization: Bearer $AKB_AUTH")
```

## File a bug you noticed (do this, then keep working on your task)

Agents often find unrelated bugs mid-task. Record them in one call and move on — don't fix them
in your current branch. The first line is the title; the rest is the body (markdown).

```sh
curl -sS -X POST "$AKB_API/projects/$P/issues" "${H[@]}" -H 'Content-Type: text/plain' --data-binary $'Crash when printing a nested optional
Where: Compiler/Generation/FnCodeGenerator.cpp:412
Repro: `👄 🤷‍♀️❗️` inside a 🍬🍬🔡 context segfaults.
Expected: prints nothing.'
```

Or JSON with labels/priority: `-H 'Content-Type: application/json' -d '{"title":"…","body":"…","labels":["bug"],"priority":"P2"}'`.
The response contains `number` and `url`. New issues land in **triage** with the `found-by-agent` label.

## Look around

```sh
curl -sS "$AKB_API/runs/current" "${H[@]}"                 # your run: issue, PR, branch, worktree, what's expected of you
curl -sS "$AKB_API/projects/$P/issues/{{ issue or "N" }}" "${H[@]}"      # issue + comments + PRs + allowed_transitions
curl -sS "$AKB_API/projects/$P/issues?q=optional&state=triage" "${H[@]}"  # search (check for duplicates before filing)
```

## Comment

```sh
curl -sS -X POST "$AKB_API/projects/$P/issues/{{ issue or "N" }}/comments" "${H[@]}" -H 'Content-Type: application/json' \
  -d '{"body":"Root cause: … Plan: …"}'
```

## Move your issue

```sh
curl -sS -X POST "$AKB_API/projects/$P/issues/{{ issue or "N" }}/transition" "${H[@]}" -H 'Content-Type: application/json' \
  -d '{"to":"in_review","comment":"Ready for review: tests pass (see PR)."}'
```

States: `triage`, `backlog`, `ready`, `in_progress`, `changes_requested`, `merge_conflict`, `in_review`,
`ready_to_merge`, `done`, `closed`. A 409 response lists the `allowed_transitions` for you.

| Role | Allowed moves |
|---|---|
| triage | `triage → ready / backlog / closed` (closing needs `close_reason`: duplicate, invalid, wontfix) |
| fix | `in_progress / changes_requested / merge_conflict → in_review` (needs an open PR with commits) |
| merge_prep | `merge_conflict → in_review` |
| review | none directly — post a verdict (below), which moves the issue |

Triage can also set fields: `PATCH …/issues/{{ issue or "N" }}` with `{"priority":"P1","size":"S","add_labels":["bug"]}`.

## Ask a human to decide

Only for real language/design decisions, and **only if the answer is not already in the thread**
(look for comments with `"kind": "decision"`). The issue pauses until a human answers.

```sh
curl -sS -X POST "$AKB_API/projects/$P/issues/{{ issue or "N" }}/decision-request" "${H[@]}" -H 'Content-Type: application/json' -d '{
  "question": "Should 🍨 setters copy-on-write per element or once per loop?",
  "options": ["Per element (current, simple)", "Once per loop via a new API"],
  "consequences": "Option 2 enables vectorization but adds public API surface."}'
```

Then end your turn.

## Pull requests (fix agents)

Commit on your worktree branch (follow the project's commit convention), then:

```sh
curl -sS -X POST "$AKB_API/projects/$P/pulls" "${H[@]}" -H 'Content-Type: application/json' -d '{
  "title": "🦁 Fix sorting edge cases",
  "body": "Fixes the comparator overflow.\n\nTests: `python3 tests.py` — 412 passed.\n\nDependencies: none."}'
```

`branch` and `issues` default to your run's branch and issue. If a PR is already open for your
branch, just commit — the PR picks up new commits automatically. Don't push or use GitHub.

Addressing review threads:

```sh
curl -sS "$AKB_API/projects/$P/pulls/N/threads?resolved=false" "${H[@]}"
curl -sS -X POST "$AKB_API/threads/THREAD_ID/replies" "${H[@]}" -H 'Content-Type: application/json' -d '{"body":"Fixed in abc1234: now checks bounds first."}'
```

Reviewers resolve threads; fix agents reply. Then move the issue to `in_review`.

## Reviews (review agents)

```sh
curl -sS "$AKB_API/projects/$P/pulls/N" "${H[@]}"                       # PR, comments, reviews, threads, head_sha
curl -sS "$AKB_API/projects/$P/pulls/N/diff" "${H[@]}"                  # full diff vs merge base
curl -sS "$AKB_API/projects/$P/pulls/N/diff?since=LAST_REVIEWED_SHA" "${H[@]}"  # only what changed since your last review
```

Inline comment on a line of the new code (`side` `RIGHT`) or removed code (`LEFT`):

```sh
curl -sS -X POST "$AKB_API/projects/$P/pulls/N/threads" "${H[@]}" -H 'Content-Type: application/json' -d '{
  "path": "Compiler/Types/Type.cpp", "line": 214, "side": "RIGHT", "severity": "blocking",
  "body": "This dereferences `generic` before the null check on L210."}'
curl -sS -X POST "$AKB_API/threads/THREAD_ID/resolve" "${H[@]}" -H 'Content-Type: application/json' -d '{"comment":"Verified fixed."}'
```

`severity` is `blocking` (must be resolved before approval) or `nit`.

Record exactly one verdict for the head you reviewed:

```sh
curl -sS -X POST "$AKB_API/projects/$P/pulls/N/reviews" "${H[@]}" -H 'Content-Type: application/json' -d '{
  "verdict": "changes_requested",
  "commit_sha": "HEAD_SHA_YOU_REVIEWED",
  "body": "Two blocking issues (see threads). Tests pass otherwise."}'
```

| verdict | effect |
|---|---|
| `approve` | issue → `ready_to_merge` (a human merges). Requires no unresolved blocking threads. |
| `changes_requested` | issue → `changes_requested`; the fix agent picks it up |
| `needs_decision` | issue paused for a human; `body` must hold the question, options and consequences |
| `comment` | no state change |

If the head moved since you started, you get a 409 — re-review the new commits with `diff?since=`.

## Errors

Errors are `application/problem+json`: `{"type","title","status","detail","allowed_transitions"?}`.
Read `detail` — it says what to do next.
