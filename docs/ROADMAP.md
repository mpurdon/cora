# Roadmap

Things considered and deliberately deferred, with enough of the reasoning that
picking one up later doesn't start from scratch. Not a backlog of chores — each
entry is a decision waiting on something.

## TypeSafe Jev as a sidecar judge

**Status:** evaluated 2026-09-22, not started. Blocked on a data-boundary answer.

[Jev](https://docs.typesafe.ai) is a "System One" model: ~150 ms, $0.042/Mtok,
text only, no generation. You send a `state` and typed questions — Choice (pick
an option), Score (rate on a rubric), Noul (is this true?) — and get back typed
answers with calibrated probabilities and a `confidence` you can threshold on.
State is capped at 32k tokens per request and accuracy drops when the state
carries material the question doesn't need.

It can't replace any pass Cora has: the assessment, the architecture graph,
findings with suggestions, chat and the drafting in the user's voice all
generate text or structure, which Jev does not do. What it fits is the class of
gut-check judgments Cora currently makes with regexes, heuristics, or by
spending an Opus turn.

Ranked by what they'd be worth:

1. **Verifying findings.** Run-to-run variance in code-pass findings is large
   (see the effort experiment). For each `CodeFinding`, state = the finding, its
   suggestion, and the anchored hunk with surrounding context; ask whether the
   cited code actually exhibits the described defect, plus a severity Score.
   Low-confidence findings get demoted or marked unverified rather than shown as
   fact. Fractions of a cent per finding, all of them in parallel, and the
   Experiments pane can measure the precision change against the bench PRs.
2. **Routing before the expensive pass.** From title, body, file list and diff
   metrics: dependency bump / generated / docs-only / test-only, and whether the
   PR touches auth, money, a schema or an external boundary. That replaces
   `isRoutineBump` and could set the architecture pass's effort, which the
   effort experiment showed is the lever that matters.
3. **Comment semantics.** `is_non_blocking_comment` is a prefix regex. "Does
   this comment require action before merge?", "is this thread addressed?" are
   one-hop judgments that feed the approve gate, the Teams seed, and callout
   importance.
4. **Per-file boundary classification in the scout** for large diffs. Jev can't
   name feature slices, so the Haiku scout stays, but the boundary flags are a
   Choice per file.

**The blocker:** Cora sends code only to Bedrock inside the company AWS account.
Jev is third-party SaaS at typesafe.ai — a new vendor and a new data boundary
(they advertise no training on requests, and ZDR for enterprise, but that's a
policy conversation, not a config flag). For public repos it's a non-issue; for
work PRs it needs an answer first, and the corporate proxy may block the
endpoint regardless. Settle that before writing code.

**Notes for whoever picks it up:** no Rust SDK, so it's `reqwest` against
`POST /v1/systemone`; rate limits are documented as changing without notice;
start with (1) behind a setting and wire it into the Experiments pane so the
comparison is measured rather than eyeballed. Docs mirror:
`~/.claude/projects/reference/typesafe-jev/`.

## Messaging the author on Teams via Microsoft Graph

**Status:** parked. The shipped route (Power Automate webhook, or a deep link
when no webhook is configured) covers the need without it.

The Graph route — `POST /chats` then `POST /chats/{id}/messages` with delegated
`Chat.ReadWrite` — would drop the Power Automate flow the user has to build by
hand, and would reuse the device-code auth already written on the dormant
`teams-pr-mentions` branch. It needs an Entra app registration (a client id from
IT) that has been outstanding since July 2026. If that ever lands, it slots into
`src-tauri/src/teams.rs` as a third sender beside the webhook and the deep link;
ask for `Chat.ReadWrite` so there's no second round-trip.
