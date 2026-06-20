# eval-core — Roadmap

Status: **v0.1.0 published to crates.io.** The core is shipped — the `Agent`/`Scorer`/`Harness`
traits, the built-in `Expectation` assertion catalog, generic `EvalCase`/`load_cases`
(single- and multi-case RON files), `run_suite`/`run_eval`, the `EvalReport` + self-contained HTML
report, an 18-case `baseline()` suite, README + a compiling crate doctest, and green CI
(`lint` on stable; `build`/`test`/`package` on stable + MSRV 1.88).

This file tracks the outstanding work. `[ ]` = planned, `[x]` = done. Each item notes *why* and
*where to start* so it can be picked up cold.

---

## Near-term polish (small, high value)

- [ ] **README badges** — CI status, crates.io version, docs.rs. Cheap credibility signal. (`README.md`)
- [ ] **`CHANGELOG.md`** — start one now (Keep a Changelog format) so 0.2.0 has a home.
- [ ] **CI: bump `actions/checkout@v4` → `v5`** — silences the "Node 20 deprecated" annotation in
  every run. Purely cosmetic today; GitHub auto-runs v4 on Node 24. (`.github/workflows/ci.yml`)
- [ ] **Supply-chain CI** — add `cargo deny check` (licenses/advisories) and/or `cargo audit` as a
  job. Low effort, good hygiene for an OSS dependency.

## Built-in assertions (`src/expect.rs`)

- [ ] **LLM-as-judge assertion** — the headline future feature. A `JudgedBy { rubric }` variant that
  grades open-ended answers (language quality, reasoning) with a model, for cases that string/number
  matching can't cover. **Deliberately deferred from v0.1**: it reintroduces an LLM-client dependency,
  so it must live behind an optional `judge` feature and take a user-supplied grader (don't hardcode a
  provider). Keep the core dependency-light.
- [ ] **More matchers as needs arise** — candidates: `FinalTextNotContains`, `FinalJsonMatches`
  (final text parses to JSON matching a subset, reusing the existing `json_subset_matches`),
  `ToolCalledBefore/After` (finer ordering than `CalledToolsInOrder`), and budget assertions
  `LatencyUnder { ms }` / `TokensUnder { n }` (promote the report-level latency/token data to
  per-case predicates). Add only when there's a real use case; keep the enum focused.
- [ ] **Case tags/metadata** — optional `tags: Vec<String>` on `EvalCase` for grouping/filtering in
  reports (e.g. run only the `math` cases). Touches `src/case.rs` + the report.

## Harness ergonomics — "batteries included" (optional feature)

- [ ] **Ship a ready-made agent loop.** Today users implement `Agent::run` over their own loop. Add an
  optional feature (e.g. `openai`) providing a `ChatBackend` trait + OpenAI-shaped DTOs + a default
  tool-calling `Agent` so anyone hitting an OpenAI-compatible API gets a working harness for free.
  **Caveat:** keep it strictly generic (no game/provider assumptions) and feature-gated so the core
  stays dependency-light. (This was scoped but not built; the reference loop lives in the AetherCore
  `ai_harness` crate if you want a starting point.)

## Results upload & integrations (the OSS half of the hosted story)

- [ ] **`upload` feature** — behind an `upload` feature, POST a `RunRecord` to a configurable backend
  endpoint with an API key + project/run identity, plus offline-queue/later-sync. The `RunRecord`
  JSON is already the wire format. This is the generic client side; it should work against *any*
  compatible backend (a hosted companion dashboard is planned — see below). Keep `reqwest` etc. behind
  the feature so the default build stays lean.
- [ ] **(stretch) OpenTelemetry export** — an alternative to the bespoke upload, emitting runs/traces
  via OTLP so results can flow into existing observability backends (Langfuse, etc.). Evaluate vs the
  simple `upload` once that exists.
- [ ] **Machine-readable CI output** — JUnit XML (and/or a stable JSON schema) from `EvalReport` so
  eval suites plug into CI test reporters and PR annotations.

## Reporting (`src/report.rs`, `src/report_html.rs`)

- [ ] **Run-over-run diff / trends** — the HTML report already does a model×case leaderboard; add a
  regression view (this run vs last) and a trend line across stored runs. Most useful once `upload`
  exists to persist history.

## Pre-1.0 API review (do before tagging 1.0 — the API is public and semver-bound)

- [ ] **`Scorer::score` label allocation** — returns `(String, bool)`, allocating a label per predicate
  even on pass. If profiling over large suites shows pressure, switch to `Cow<'static, str>`. Low
  priority. (reviewer note)
- [ ] **`RunMeta`'s LLM-flavored fields** (`temperature`/`backend`/`system_prompt`) sit on an otherwise
  domain-agnostic core. Acceptable today (optional, neutral defaults); reconsider whether they belong
  on a generic type or in an extension before locking the API. (reviewer note)
- [ ] **Error-type stability** — `EvalError` (thiserror) is the public error surface; confirm its
  variants are what we want to commit to at 1.0.
- [ ] **`#[non_exhaustive]` audit** — already applied to `RunArtifacts`/`RunMeta`/`ToolCall`/
  `CaseOutcome`; confirm coverage of any other struct likely to gain fields.

## Companion product (separate, not part of this OSS crate)

A hosted results dashboard ("eval-forge") is planned as a companion service: it would ingest the
`RunRecord`s sent by the `upload` feature above and provide storage, comparison, and real-time
monitoring. The OSS integration point is the `upload` feature; the service itself is tracked
separately.

---

### How to cut a release

See `PUBLISHING.md`. In short: bump `version` in `Cargo.toml`, update `CHANGELOG.md`, `cargo publish`,
then (if you use it downstream) bump the `eval-core = "0.x"` requirement in the consuming project.
