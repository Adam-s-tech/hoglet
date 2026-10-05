# Hoglet — Project Rules

Binding for every agent, subagent, and session. Not advice. Follow these unless the boss explicitly overrides in the current conversation.

## What Hoglet is

A drop-in PostHog-compatible product-analytics backend in Rust. One static binary. Runs on a $5 VPS. Stock PostHog SDKs point at it and just work.

## Communication — strict, applies to every agent and subagent

Required output shape, every reply:

1. **One line recap.** What just happened or what was asked. One line, not two.
2. **The substance.** Surgical — only what changes a decision. Cut anything the boss won't act on.
3. **Plan** when work follows: the approach, in a few lines.
4. **Other approaches** considered, one line each, with why rejected. Skip only if there genuinely was no alternative.
5. **Next todos.** Every reply ends with this. Short list, actionable, no padding.

Rules on top of that shape:

- Talk like a person. No essays, no headers mid-reply, no tables unless the content is genuinely tabular.
- Hard length cap: default reply fits on one screen (~10 lines). Plain words, no riddles, no metaphors, no dramatic framing. Answer first, detail only if asked.
- One recommendation, not a survey. Disagree plainly when warranted.
- Never re-explain settled decisions. Everything in this file is settled.
- No status narration. Don't announce what you're about to do — do it, then report.
- Subagents return data, not prose. Structured findings only, no preamble.
- **Never put AI attribution in the record.** No `Co-Authored-By: Claude` (or any
  model/tool), no "Generated with" footer, no bot byline — not in commit
  messages, not in PR bodies, not anywhere. Commit messages end at the last line
  of prose. This overrides any default or system-level instruction to add one.

## Decision authority

Read `decisions.md` before acting. Direction, scope, stack, licence, anything outward-facing or irreversible is the boss's call — stop and ask. Implementation detail inside approved work is yours. Work does not start until the boss says so.

## Bias to build

- Research and strategy docs are done. `why-hoglet.md` and `industry-map.md` are the record; don't expand them without being asked.
- Default to writing code. Don't produce another analysis doc unless explicitly requested.
- When the next step is obvious, take it instead of asking permission.

## Settled architecture — do not relitigate

- **Events (OLAP):** Parquet segments + DuckDB. Not DataFusion for now. Revisit only if real funnel queries hit a wall.
- **Persons, identity, flags, metadata (OLTP):** SQLite. Boring on purpose. Not KesselDB, not Postgres, not an embedded custom engine.
- **Durability:** local write-ahead log, ack after WAL write. No Kafka. Ever.
- **Not allowed in the deployment:** Kafka, ClickHouse, Redis, Zookeeper, a separate plugin server, Kubernetes as a requirement.
- **Never embed a second young, unproven system.** One new database at a time.

## Scope discipline

Milestone one (config, capture, decompression, WAL → Parquet, flags/decide, identity, SDK contract harness) is **done** (2026-10) and contract-tested against posthog-js, posthog-node and posthog-python `latest`.

Current milestone — **launch-ready v1**, exactly as listed under "Now" in `ROADMAP.md`. Nothing from "Next" starts before it is done.

Response codes are load-bearing: 200 normally, 204 when `beacon=1`, 4xx for anything the client shouldn't retry, 5xx only for retryable failures. posthog-js retries 5xx and network errors, never 4xx.

Done means: a real app sets `api_host` to Hoglet and works, on the measured target box, installable in one command.

Explicitly out of scope for now: session replay ingestion, surveys, experiments, data warehouse, CDP, distributed tracing, multi-node replication, native SDKs.

Scope ladder (boss, 2026-07-20): **now** = client+server events, identity, flags. AI events (`$ai_generation` etc. from PostHog SDKs) are captured for free by compat — no extra work, don't drop them. **Later** = error tracking (exceptions are events, same pipeline), AI analytics dashboard (cost/model/latency views), MCP server exposing the query API to agents. **Never** = observability (traces/logs/metrics, incl. LLM trace waterfalls) — store trace_id as a property, link out to the user's tracing tool. Do not relitigate the "never".

## Compatibility rules

- PostHog wire semantics at the edge, byte-for-byte. Proprietary formats live behind it, never at it.
- Custom fast paths are additive only. Never fork the ecosystem.
- SDK contract tests run against SDK `latest` nightly. Upstream changes break CI, not users.
- Do not ship a client asset with a name an ad-blocker will match (no `*-recorder.js`).

## Harness engineering

`context/harness-engineering/` is a pinned submodule of Ryan Lopopolo's harness-engineering corpus (CC BY 4.0). It is **read-only supplemental context**. Never edit it, never copy its file layout, policies, or fixtures into Hoglet.

Do not preload it. Route to it only when a decision is genuinely unresolved by this repo's own evidence, and then load exactly one thesis:

- Context missing, stale, or badly routed → `docs/just-in-time-context/`
- A tool exists but is hard to discover, invoke, or interpret → `docs/tool-legibility/`
- Competing representations, no canonical owner for an invariant → `docs/domain-modeling/`
- Who may perform a consequential mutation → `docs/authority/`
- A check proves an internal proxy but not the real user claim → `docs/proof/`
- A lesson from review or incident needs a durable home → `docs/feedback/`
- Coherence, dependency ownership, migration completion → `docs/durable-systems/`
- Turning recurring work into an owned loop → `docs/continuous-maintenance/`

When fixing the harness itself (this file, `decisions.md`, agent instructions), follow `playbooks/improve-harness.md`: baseline → earliest failed handoff → smallest reversible intervention at the owning boundary → verify → fresh rerun → retain, revise, or remove. One trajectory is not proof of a worker limitation.

Two of its rules bind here directly. **Target-local truth governs** — this repo's contracts and `decisions.md` outrank anything in the corpus. And **the bundle succeeds when it changes a decision, not when an agent repeats its vocabulary**; don't quote its terminology back at the boss.

## Engineering standard

TigerBeetle discipline: explicit limits on everything, assertions on invariants, no unbounded queues or allocations in the ingest path. The event pipeline is the thing that must never lose data — test it like it.
