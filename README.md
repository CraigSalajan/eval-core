# eval-core

**pytest/jest, but for LLM agents.** Write test cases whose inputs are *prompts* and whose
assertions are built-in checks on what the agent *did* — which tools it called, with which
parameters, and what it finally said or computed.

`eval-core` is an agent testing framework: prompts in, assertions on behavior out; bring your own
harness. You implement ONE method that runs a prompt against your agent and returns what it did;
you write a tiny suite of assertions (in RON or inline); the framework runs every case, times it,
isolates panics, and hands you a report (plus a self-contained HTML dashboard). It is
game-agnostic and has zero dependency on any LLM/engine crate, so a case file is just a prompt and
a list of expectations.

---

## Quickstart (the 5-minute path)

### 1. Implement `Agent` over your harness

One method: run a prompt, return what the agent did as [`RunArtifacts`]. Build the artifacts with
the chainable `with_*` builders and `ToolCall::new`.

```rust
use eval_core::{Agent, EvalError, RunArtifacts, ToolCall};
use serde_json::json;

struct MyAgent; // wraps your real LLM loop / tools

impl Agent for MyAgent {
    fn run(&self, instruction: &str) -> Result<RunArtifacts, EvalError> {
        // ... drive your real agent here, collecting the tool calls it made
        // and its final reply. (Return `Err(EvalError::agent(...))` on a backend failure.)
        Ok(RunArtifacts::new()
            .with_tool_calls(vec![
                ToolCall::new("calculator", json!({ "op": "add", "a": 2, "b": 2 })),
            ])
            .with_final_text("The answer is 4.")
            .with_tokens(17)) // optional; forwarded to the report's token stats
    }
}
```

Every `RunArtifacts` field is optional — a minimal agent can return
`RunArtifacts::new().with_final_text(...)` and nothing else.

### 2. Write a RON suite of built-in `Expectation`s

A case is a `name`, an `instruction` (the prompt), and a list of `expect` predicates. A file may
hold a single case or a list of them:

```ron
[
    (
        name: "adds-two-numbers",
        instruction: "what is 2 + 2?",
        expect: [
            CalledToolWith(tool: "calculator", args: { "op": "add" }),
            FinalNumberEquals(value: 4.0),
        ],
    ),
    (
        name: "no-tools-for-chitchat",
        instruction: "hello there",
        expect: [
            NoToolCalls,
            FinalTextContains(text: "hello", case_insensitive: true),
        ],
    ),
]
```

There is no `setup` field on the easy path — it defaults to `()`.

### 3. Run the suite and read the report

```rust
use eval_core::{load_cases, run_suite};
use eval_core::report_html::generate_report;
use std::path::Path;

# fn demo(agent: &impl eval_core::Agent) -> Result<(), eval_core::EvalError> {
let cases = load_cases(Path::new("suite/"))?; // every *.ron in the dir, sorted
let report = run_suite(agent, &cases);

println!("{report}"); // the human-readable summary table (progress goes to stderr)

// ...or persist run records and render a self-contained dashboard:
let _html = generate_report(Path::new("results/")); // writes results/report.html
# Ok(())
# }
```

That's the whole loop: `Agent::run` → author `Expectation`s → `run_suite`. No `World`, no
`Setup`, no `Scorer` to implement.

> Prefer inline cases for a first run? Build a `Vec<EvalCase<(), Expectation>>` directly — see
> [`examples/calculator.rs`](examples/calculator.rs) for a complete, dependency-free agent (no real
> LLM) tested entirely with the built-in assertions.

---

## Run the shipped baseline

`eval-core` ships a ready-to-run baseline capability suite (18 cases). Hand it straight to
`run_suite`:

```rust
# fn demo(agent: &impl eval_core::Agent) {
let report = eval_core::run_suite(agent, &eval_core::baseline());
println!("{report}");
# }
```

The baseline covers three capability areas:

| File             | Cases | What it checks                                                          |
|------------------|-------|-------------------------------------------------------------------------|
| `arithmetic.ron` | 6     | Mental/calculator math (number assertions, plus an opt-in tool subset). |
| `language.ron`   | 5     | Instruction-following over the agent's final text (fully portable).     |
| `tool_use.ron`   | 7     | Tool-calling with the right name/args/count/order.                      |

**Portable vs. adapt-me, honestly:** the number/text assertions only inspect the agent's final
reply, so they hold whether the agent uses tools or reasons inline. The `tool_use.ron` cases (and
a small, clearly-labelled subset of `arithmetic.ron`) additionally assert *specific tool names and
args* — they assume a documented convention (a `search`, `calculator`, and `send_email` tool) that
you must adapt to your own agent. Do not read a tool-name mismatch there as a capability failure.

To start from the baseline as a *template*, dump the raw embedded files into your own suite
directory and edit them — same files that `baseline()` runs:

```rust
# fn demo(my_suite_dir: &std::path::Path) -> std::io::Result<()> {
for (name, contents) in eval_core::baseline_files() {
    std::fs::write(my_suite_dir.join(name), contents)?;
}
# Ok(())
# }
```

---

## Assertion catalog

Every built-in `Expectation` variant, its exact RON form, and what it asserts over the run's
`RunArtifacts`. A case **passes iff the run did not error AND every expectation holds.**

| Variant | RON syntax | Asserts |
|---------|-----------|---------|
| `CalledTool` | `CalledTool(tool: "search")` | The agent called `tool` at least once (any args). |
| `DidNotCallTool` | `DidNotCallTool(tool: "search")` | The agent never called `tool`. |
| `CalledToolWith` | `CalledToolWith(tool: "calc", args: { "op": "add" })` | The agent called `tool` at least once with args that **superset** the given `args` (subset match — see below). |
| `ToolCallCount` | `ToolCallCount(tool: Some("search"), min: Some(1), max: Some(1))` | The number of calls is within `[min, max]` (each optional). `tool: Some(name)` counts only that tool; omit `tool` (defaults `None`) to count **all** calls. Either bound may be omitted. |
| `CalledToolsInOrder` | `CalledToolsInOrder(tools: ["search", "send_email"])` | The named tools appear as a **subsequence** of the call order — in this relative order, but not necessarily contiguous (other calls may interleave). Empty `tools` trivially holds. |
| `NoToolCalls` | `NoToolCalls` | The agent made **no** tool calls at all (pure-reasoning / refusal check). |
| `FinalTextContains` | `FinalTextContains(text: "paris", case_insensitive: true)` | `final_text` contains `text`. `case_insensitive` defaults to `false` (exact-case substring). Fails when there is no final text. |
| `FinalTextEquals` | `FinalTextEquals(text: "OK")` | `final_text` equals `text` exactly, after trimming surrounding whitespace on both sides. Fails when there is no final text. |
| `FinalTextMatches` | `FinalTextMatches(regex: "(?i)^\\s*(yes\|no)\\b")` | `final_text` matches `regex` anywhere. Fails when there is no final text. A **malformed regex is an authoring error** (a hard `EvalError::Regex`), surfaced as a clearly-labelled failed predicate rather than a silent miss. |
| `FinalNumberEquals` | `FinalNumberEquals(value: 3.33, tolerance: 0.01)` | The **last number** in `final_text` equals `value` within `tolerance` (see below). `tolerance` defaults to `0.0` (exact). Fails when there is no final text or it contains no number. |
| `NoError` | `NoError` | The run reported no error. (The runner already fails a case on any run error; this is an explicit, labelled "the run was clean" predicate.) |

### Two matching rules to know

- **`CalledToolWith` args are a SUBSET match.** The expected `args` JSON must be a subset of the
  actual call's args: objects recurse key-by-key, and every other JSON value
  (string/number/bool/null/**array**) must match exactly. So `{ "op": "add" }` matches a call made
  with `{ "op": "add", "a": 2, "b": 2 }`, but `{ "op": "sub" }` does not, and an extra key the call
  didn't have (`{ "op": "add", "c": 9 }`) does not. **Arrays are compared whole**, not
  element-subset — `{ "at": [1, 2] }` does *not* match a call with `"at": [1, 2, 3]`. This keeps
  positional args like a `[x, y, z]` coordinate predictable.
- **`FinalNumberEquals` extracts the LAST number.** The last numeric token in `final_text` is taken
  as the agent's answer (models typically end with the answer), then compared to `value` within
  `tolerance`. "Number" = an optionally-signed integer or decimal, with ASCII thousands-separator
  commas tolerated inside the integer part (`-1,024.50` → `-1024.5`). A lone `-`/`.` is not a
  number.

### serde defaults

- `FinalTextContains.case_insensitive` → `false`
- `FinalNumberEquals.tolerance` → `0.0` (exact)
- `ToolCallCount.tool` / `.min` / `.max` → `None` (no restriction / unbounded)

---

## Authoring cases

- **One file, one or many cases.** A `.ron` file may hold *either* a single `EvalCase(...)` *or* a
  list `[EvalCase(...), EvalCase(...)]` of related cases. Group a whole capability in one file
  without one-file-per-case sprawl.
- **`setup` defaults to `()`** on the easy path, so a case omits it entirely (see the examples
  above). On the advanced path it defaults to your `Setup::default()`.
- **`load_cases(dir)`** reads every `*.ron` in `dir` in **sorted (filename) order** — deterministic
  across runs and machines — and flattens single- and multi-case files together, so a directory may
  freely mix the two shapes. Non-`.ron` entries and subdirectories are ignored.
- **Fail-loud.** A malformed `.ron` is a hard `EvalError` naming the offending file (so a typo fails
  the load, and any CI load test, rather than silently dropping a case).

---

## Advanced: domain-state assertions

The easy path scores what the agent *did* (tool calls, params, final text/number). When you need to
assert on **state the agent actually changed** — not just the calls it emitted — there is an escape
hatch:

1. Implement [`Harness`] over your own `World` + `Setup`: `setup(&setup) -> World` builds a fresh
   world per case, `run(&instruction, &mut world) -> Result<RunArtifacts>` runs the prompt against
   it (mutating the world).
2. Implement a custom [`Scorer`] over the **same** `World`: `score(expect, &artifacts, &world)`
   returns `(label, passed)` for each of a case's predicates, inspecting the post-run world.
3. Run with [`run_eval`] (or `run_eval_with_meta`) instead of `run_suite`.

```rust
use eval_core::{Harness, RunArtifacts, Scorer};

struct MyHarness;
impl Harness for MyHarness {
    type World = World;       // whatever state your agent mutates
    type Setup = Setup;       // per-case starting configuration
    fn setup(&self, setup: &Setup) -> World { /* build a fresh world */ }
    fn run(&self, instruction: &str, world: &mut World) -> anyhow::Result<RunArtifacts> {
        /* run the prompt against `world`, return what the agent did */
    }
}

struct MyScorer;
impl Scorer for MyScorer {
    type World = World;
    type Expect = MyPredicate; // your own predicate type
    fn score(&self, expect: &MyPredicate, _artifacts: &RunArtifacts, world: &World) -> (String, bool) {
        /* inspect `world` to decide if the predicate held */
    }
}
```

This is the advanced ~10%. The reference consumer (the AetherCore voxel engine) uses it for
"did the world actually change" checks — e.g. asserting *N* solid voxels were placed after a build
command, by scoring the post-run voxel world rather than the agent's tool calls. See
[`examples/minimal.rs`](examples/minimal.rs) for a complete, dependency-free `Harness` + `Scorer`.

---

## Output

`run_suite` / `run_eval` return an `EvalReport`:

- **Accuracy** — `passed()/total()` cases, plus `accuracy()` in `0.0..=1.0`.
- **Latency** — `mean_latency()`, `p50_latency()`, `p95_latency()` (nearest-rank quantiles).
- **Tokens** — `total_tokens()` / `mean_tokens()` over cases that reported a count (`None` when none
  did, so the summary never prints a misleading `0`).
- **Per-case detail** — each `CaseOutcome` carries `passed`, the per-predicate `(label, passed)`
  list (pinpointing *which* predicate failed), `latency`, `tokens`, the tool-call display strings,
  `final_text`, any run `error`, and the transcript.

`println!("{report}")` prints an aligned summary table with the failed predicates spelled out per
case. For comparing many models/runs at a glance, persist `RunRecord`s as JSON to a directory and
call `report_html::generate_report(dir)` — it writes a single, **fully self-contained** `report.html`
(no CDN, no external scripts; opens offline by double-click) with a sortable leaderboard, a
model × case heatmap, and per-case transcript expanders.

---

## Install

```sh
cargo add eval-core
```

This is a pre-release `0.1` — the API may still shift between minor versions.

## License

MIT OR Apache-2.0.

## Status / roadmap

Young but usable; the next planned step is a hosted results dashboard on top of the existing
self-contained HTML report.
