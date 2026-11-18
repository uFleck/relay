# JEV client and routing

**Status:** complete, awaiting review.
**Agent / model:** DeepSeek implementation sub-agent (DeepSeek V4.1 Flash via T3 Code / OpenCode).
**Commits:** this commit — subject `[deepseek] Add JEV client and routing thresholds`, parent `9900a1b` (`[maintenance] Format src-tauri and record release measurement`, 2026-11-17). A commit cannot carry its own hash inside its tree; the hash is recorded in the handoff and in `git log`.
**Files:**

- `crates/relay-core/src/jev.rs` (new): config, client, questions, answers, routing and tests.
- `crates/relay-core/src/lib.rs`: exports `jev`.
- `crates/relay-core/Cargo.toml`: adds `reqwest`, `rustls` and the two dev-dependencies.
- `Cargo.lock`: the workspace lockfile, updated only by those dependency additions.
- `docs/reviews/2026-11-17-jev-client.md`: this record.

It does **not** touch `src-tauri/`, `src/`, `design/` or `docs/reviews/README.md`.

## What changed

`jev.rs` is a self-contained HTTP client plus a pure routing helper. It follows
`docs/PLAN.md` section 3.2 and the documented System One API
(`https://docs.typesafe.ai/api/system-one.md`, `https://docs.typesafe.ai/primitives`).
No request or response field was invented.

**Config and client**

- `JevConfig { model: "jev-latest", base_url: "https://api.typesafe.ai", timeout_secs: 15 }` with `Default`.
  serde is camelCase, so settings JSON reads `timeoutSecs`.
- `JevClient::new(api_key)` and `JevClient::with_config(api_key, config)` return `Result<_, JevError>`.
  An empty or whitespace-only key is `MissingKey` before any request. Empty `model`/`base_url` and a
  zero timeout normalize to the defaults. `base_url` is trimmed and loses its trailing slash.
- The HTTP client is `reqwest::Client` with a whole-request timeout from `timeout_secs` (15 s default),
  rustls TLS and gzip. `POST {base_url}/v1/systemone`, `Authorization: Bearer <key>`,
  `Content-Type: application/json`.
- The key is wrapped in a private `ApiKey` whose `Debug` is `<redacted>`, and `JevClient` has a manual
  `Debug` that prints the redacted value. Errors carry `reqwest`/status/decode text only; none of them
  can contain the key.

**Questions** (`serde(tag = "type", rename_all = "snake_case")`)

- `Question::Noul { instructions: Option<Value>, criteria: Option<NoulCriteria> }`
- `Question::Choice { instructions: Option<Value>, criteria: BTreeMap<String, Value> }`
- `Question::Score { instructions: Option<Value>, criteria: Vec<Value> }`
- Constructors `noul(instructions)`, `noul_with_criteria(instructions, NoulCriteria { yes, no })`,
  `choice(instructions, criteria)` and `score(instructions, levels)`. A `Value::Null` instruction is
  omitted from the body rather than sent as `null`. Criteria are serialized as the documented map/list.
- The API limits Choice to 1..=255 criteria and Score to 1..=10 levels. The client documents the limits
  but does not duplicate them; an out-of-range question comes back as HTTP 422 and therefore
  `JevError::Status`.

**Answers** (`serde(tag = "type", rename_all = "snake_case")`)

- `JevResponse { model, answers: BTreeMap<String, Answer>, usage: Usage { input_tokens, output_tokens } }`.
  `model` is the concrete model the API reports, not necessarily the requested alias.
- `Answer::Noul(NoulAnswer { noul })`,
  `Answer::Choice(ChoiceAnswer { choice, confidence, probabilities })`,
  `Answer::Score(ScoreAnswer { score, confidence, legend, probabilities })`.
  Newtype variants keep the documented wire tags while letting the accessors return references to the
  named payload structs. A serde round-trip test pins the tags.
- Helpers: `answer(id)`, `noul(id) -> Option<f64>`, `choice(id) -> Option<&ChoiceAnswer>`,
  `score(id) -> Option<&ScoreAnswer>`, `usage()`.

**`evaluate(state, questions)`**

- `state` is any JSON the API accepts (string, object or array); questions go in one request.
- Errors: `MissingKey`, `Http(reqwest::Error)`, `Status { status: u16, body }`,
  `Decode(serde_json::Error)`, `Timeout`. `Timeout` is a reqwest timeout on send or body read.
- A non-2xx body is cut to 500 **characters** on a UTF-8 boundary, so a 400/422 detail is visible
  without dumping an unbounded body.

**Routing** (`route(&JevResponse, Thresholds, task_claimed) -> Route`)

`Thresholds` defaults are the PLAN table: `screen_yes 0.60`, `screen_no 0.25`, `change 0.70`,
`new_task 0.80`, `complete 0.70`, `needs_human 0.60`. Comparisons are strict `>` (or `<` for the
archive), so a value exactly on a threshold does not trigger it.

Answer ids are the documented ones: `worth_reviewing`, `conversation_state`, `affects_existing`,
`task`, `change_kind`, `is_new_task`, `conversation_complete`, `urgency`, `needs_human`, `project`
(exposed as `jev::ids` constants). `conversation_state`, `change_kind` and `urgency` are not inputs to
routing; PLAN 3.2 routes on the others.

| Condition (in evaluation order) | Route |
| --- | --- |
| `worth_reviewing` missing | `Review` (cannot screen) |
| `worth_reviewing < screen_no` | `Ignore` (archive wins even when other answers are loud) |
| `screen_no <= worth_reviewing <= screen_yes` | `Review` (middle band, rescreen at close) |
| `needs_human` missing | `Review` |
| `needs_human > needs_human` | `Review` (never auto) |
| `affects_existing` or `is_new_task` or `task` missing | `Review` |
| `affects_existing > change` **and** `is_new_task > new_task` | `Review` (both high; reviewer decides) |
| `task = multiple` | `Review` (reviewer splits, one change per task) |
| `affects_existing > change` and `task = none` | `Review` (answers conflict) |
| `affects_existing > change` and `task` is an id | `Change { task_id, early: task_claimed }` |
| `conversation_complete` or `project` missing | `Review` |
| `is_new_task > new_task` and `conversation_complete > complete` | `NewTask { project }`; `project = unsure` -> `None` |
| anything else | `Review` (middle band, rescreen at close) |

`Route` and `Thresholds` are serde types; `Route` is internally tagged on `kind`. The `reason` strings
are human-readable and mention the answer that caused the review.

## Commands and results

| Command | Result |
| --- | --- |
| `cargo test -p relay-core` | **Pass** — 39 passed, 0 failed, 0 ignored, finished in 1.01 s. 16 are new JEV tests (23 pre-existing). |
| `cargo clippy -p relay-core --all-targets -- -D warnings` | **Pass** — exit 0, no warnings, `Finished dev profile … in 1.89s`. |
| `cargo fmt --all -- --check` | **Pass** — exit 0, whole workspace (HEAD `9900a1b`). |
| `cargo check` (workspace, Tauri binary included) | **Pass** — exit 0, `Finished dev profile … in 6.99s`; the binary still type-checks with the unified reqwest features. |

One intermediate `cargo test` run failed with `E0618` because a test-local variable named `route`
shadowed the `route` function; the tests now use `decision`. That was a test bug, not a final result.

The 16 tests cover: the request (bearer header, JSON content type, model, exact state, all three
question shapes) and a response with a Noul, Choice and Score answer plus usage; question serde shapes
and omission of null instructions; a 400 with a 600-character body (status 400, body 500 chars, no key
in the message); a slow server mapped to `Timeout`; malformed JSON and a missing `usage` mapped to
`Decode`; the routing table (ignore, claimed/unclaimed change, new task, `unsure` project, needs-human,
multiple, both high, conflicting `none`, middle band, missing answers, strict boundaries, custom
thresholds); answer tag round-trips; `Debug` redaction; `MissingKey`; and config normalization.

## Dependency note

The task asked for `features = ["rustls-tls", "json", "gzip"]`. reqwest 0.13.5 has **no `rustls-tls`
feature**; its rustls features are `rustls` (aws-lc-rs) and `rustls-no-provider`. `rustls` pulls
`aws-lc-sys`, which needs NASM on Windows; `where.exe nasm` finds nothing on this machine. The
workspace already builds the ring provider for the updater, so the crate uses
`features = ["rustls-no-provider", "json", "gzip"]` plus a direct `rustls` dependency with
`features = ["ring"]`, and `with_config` installs the ring provider only when no process default is
set. This avoids a new native build and keeps one TLS provider in the binary.

## Known risks

- **Thresholds are uncalibrated.** They are the PLAN defaults, not measured values. Shadow mode over the
  14-day backfill is still the calibration path; a wrong threshold changes how much reaches the queue.
- **Token cost.** Every `evaluate` pays input tokens for the whole `state`. The ~12k-character stage-B
  transcript cap and candidate shortlist belong to the caller; this module has no batching, cache or
  cost guard. A large `state` sent per message batch is the main cost risk.
- **Prompt injection remains the reviewer's job.** JEV only classifies text. A crafted DM can lower
  `needs_human`; this client cannot detect that. The real barrier is the analyst endpoint's append-only
  toolset plus the "Slack text is untrusted data" instruction.
- **No keyring yet.** The key is a `String` in memory, passed by the caller. Windows Credential Manager
  integration belongs to the settings/keyring package; this client never persists, logs or returns it.
- **No retry or backoff.** A JEV outage surfaces as `Http`/`Timeout`/`Status`. PLAN says "queue and
  retry"; that is the job layer's responsibility, not this client's.
- **No rate-limit handling.** HTTP 429 is a `Status` error; `Retry-After` is not parsed.
- **`project` is required for a new task.** If a caller asks `is_new_task` but omits `project`, routing
  reviews instead of guessing the Inbox. Omitting the project question therefore removes the automatic
  path.
- **Unknown answer types fail the whole decode.** A `box` (or future) answer in the response is a
  `Decode` error even if the other answers are valid. Relay only asks Noul, Choice and Score.
- **Silent config normalization.** Empty `model`/`base_url` and zero timeout use defaults rather than
  erroring; a typo that is non-empty still fails at request time as `Http`.
- **One whole-request timeout.** There is no separate connect timeout, and a body that trickles can use
  the full 15 s.
- **Routing reasons are prose.** They are not stable machine codes; UI code should match the `Route`
  variant, not the `reason` text.

## Review checklist

1. Compare every question and answer serde name with the System One OpenAPI schema (`type`, `criteria`,
   `legend`, `probabilities`, `usage`), including the required fields and the `true`/`false` Noul criteria.
2. Confirm the `Authorization: Bearer` header, the endpoint path, and that `Debug`/`Display` of the client
   and every error can never include the key.
3. Check the error mapping: 400/422 leaves `Status` with a 500-character body, a slow server is `Timeout`,
   non-JSON or a missing required field is `Decode`, and nothing panics on a bad response.
4. Walk the routing table against PLAN.md 3.2, especially the order (archive first, `needs_human` veto,
   both-high before either automatic branch), strict `>` boundaries and `early = task_claimed`.
5. Confirm every missing-answer path reviews and never auto-applies.
6. Check the dependency choice (`rustls-no-provider` + ring, provider installed only when absent) against
   the workspace's updater, and confirm the lockfile change is only for these dependencies.
7. Confirm the tests use a fake key and a loopback axum mock only; no real key, no external network, no
   recorded secret in output.
8. Confirm the commit contains only `crates/relay-core`, `Cargo.lock` and this record — no `src/`,
   `src-tauri/`, `design/` or review-index edits.

## Opus verdict

_Pending._

## Opus verdict (2026-11-17)

**Verdict:** approved with notes
**Reviewed:** `6b2bad9` (`crates/relay-core/src/jev.rs` is unchanged at HEAD; line numbers match both)

What holds:

- The API key cannot leak through `Debug` or serialization. It lives in a
  private `ApiKey` newtype whose `Debug` prints `<redacted>`
  (`crates/relay-core/src/jev.rs:89-97`). `JevClient`'s `Debug` uses that newtype
  (`:128-137`), and `JevConfig` has no key field. No `JevError` variant carries
  the key (`:100-117`). A test checks the redaction (`:1198`).
- A whole-request timeout applies and maps to `JevError::Timeout`
  (`:176-178`, `:232-238`). Error bodies are cut on a character boundary at 500
  characters (`:240-246`).
- The routing thresholds match PLAN 3.2, with strict `>` everywhere: archive
  below `0.25` (`:515`), middle band up to and including `0.60` (`:518`),
  `needs_human > 0.60` (`:528`), both high goes to review (`:546`), `multiple`
  goes to review (`:554`), high `affects_existing` with `none` goes to review
  (`:561-568`), and a new task needs `> 0.80` and `complete > 0.70` (`:578`).
  Any missing answer goes to review and never auto-applies.
- The rustls provider is installed only when none exists, and the call ignores
  an install race (`:172-174`).

What I would change:

- The middle band returns `Route::Review` (`:518-523`, `:586-591`). PLAN 3.2
  says the middle band is "rescreen at close, not an immediate queue". A caller
  that sends every `Review` to the human queue will flood it. Fix: add
  `Route::Rescreen { reason }` and return it in those two places.
- `route` takes `task_claimed: bool` before the caller knows which task JEV
  picked (`:511`). Fix: take `is_claimed: impl Fn(&str) -> bool` and call it
  with `task.choice`.
- The client has no retry, and nothing yet implements "JEV down → queue and
  retry". That belongs to P3 jobs; the job layer must treat every `JevError` as
  "queue, never apply".

Nothing blocking for this commit. Fix the `Rescreen` variant before P3 wires
`route` to jobs.

Not covered: the real JEV API is covered only by the separate live-spike
record. I did not verify answer parsing against live responses or `Thresholds`
validation (for example, `screen_no > screen_yes` is accepted).
