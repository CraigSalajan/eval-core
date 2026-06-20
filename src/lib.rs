//! `eval-core` — **pytest/jest, but for LLM agents**: a batteries-included agent testing framework
//! where a test case is a *prompt* and the assertions are built-in checks on what the agent DID —
//! which tools it called, with which parameters, and what it finally said or computed — scored over
//! the universal [`RunArtifacts`].
//!
//! Prompts in, assertions on behavior out; bring your own harness. For the common case a host
//! implements ONE method ([`Agent::run`]), authors [`expect::Expectation`] predicates (in RON or
//! inline), and calls [`run_suite`] — no `World`, no `Setup`, no [`Scorer`] impl. It is
//! game-agnostic, so it doubles as a generic result/metric data model plus a self-contained HTML
//! comparison report (a small "Weights & Biases for evals").
//!
//! ## Quickstart
//!
//! Implement [`Agent::run`] over your harness (run one prompt, return what the agent did via the
//! `with_*` builders + [`ToolCall::new`]), author [`Expectation`] cases, and call [`run_suite`]:
//!
//! ```
//! use eval_core::{run_suite, Agent, EvalCase, EvalError, Expectation, RunArtifacts, ToolCall};
//! use serde_json::json;
//!
//! // A toy agent (no real LLM): for an "add" prompt it emits a calculator tool call and ends with
//! // the sum; for anything else it just greets, making no tool call.
//! struct MyAgent;
//! impl Agent for MyAgent {
//!     fn run(&self, instruction: &str) -> Result<RunArtifacts, EvalError> {
//!         if instruction.contains("add") {
//!             Ok(RunArtifacts::new()
//!                 .with_tool_calls(vec![ToolCall::new(
//!                     "calculator",
//!                     json!({ "op": "add", "a": 2, "b": 2 }),
//!                 )])
//!                 .with_final_text("The answer is 4."))
//!         } else {
//!             Ok(RunArtifacts::new().with_final_text("Hello!"))
//!         }
//!     }
//! }
//!
//! let cases: Vec<EvalCase<(), Expectation>> = vec![
//!     EvalCase {
//!         name: "adds-two-numbers".to_owned(),
//!         instruction: "please add 2 and 2".to_owned(),
//!         setup: (), // no `setup` on the easy path — it is `()`
//!         expect: vec![
//!             Expectation::CalledToolWith {
//!                 tool: "calculator".to_owned(),
//!                 args: json!({ "op": "add" }),
//!             },
//!             Expectation::FinalNumberEquals { value: 4.0, tolerance: 0.0 },
//!         ],
//!     },
//!     EvalCase {
//!         name: "no-tools-for-chitchat".to_owned(),
//!         instruction: "hello there".to_owned(),
//!         setup: (),
//!         expect: vec![Expectation::NoToolCalls],
//!     },
//! ];
//!
//! let report = run_suite(&MyAgent, &cases);
//! assert_eq!(report.total(), 2);
//! assert_eq!(report.passed(), 2); // both cases pass
//! // `println!("{report}")` prints the human-readable summary table.
//! ```
//!
//! In practice cases are usually authored as RON and loaded with [`load_cases`]:
//!
//! ```ron
//! (
//!   name: "adds-two-numbers",
//!   instruction: "what is 2 + 2?",
//!   expect: [
//!     CalledToolWith(tool: "calculator", args: { "op": "add" }),
//!     FinalNumberEquals(value: 4.0),
//!   ],
//! )
//! ```
//!
//! `eval-core` also ships a ready-to-run [`baseline()`] suite (arithmetic / language / tool-use, 18
//! cases) you can hand straight to [`run_suite`], and [`baseline_files`] to dump it as a template.
//! See `examples/calculator.rs` for a complete, dependency-free agent-framework example, and the
//! crate `README.md` for the full assertion catalog.
//!
//! ## Isolation guarantee
//!
//! This crate depends ONLY on small third-party crates (`serde`, `serde_json`, `ron`, `regex`,
//! `thiserror`, `anyhow`, `tracing`, `chrono` (local timestamps on auto-persisted runs), and
//! `include_dir` to embed the shipped baseline suite). It has ZERO
//! dependency on any host engine/game crate, so it can be lifted into a standalone public repository
//! unchanged. The dependency arrow points one way: a host harness depends on `eval-core`, never the
//! reverse.
//!
//! ## Modules
//!
//! - [`report`] — the result/metric data model: [`report::RunRecord`], [`report::EvalReport`],
//!   [`report::CaseOutcome`], with a readable `Display` summary and the aggregate statistics
//!   (accuracy, latency percentiles, token totals).
//! - [`report_html`] — the self-contained HTML report generator ([`report_html::generate_report`]):
//!   loads persisted [`report::RunRecord`]s from a directory and writes a single offline `report.html`.
//! - [`persist`] — automatic run persistence ([`persist::save_and_report`] / [`persist::save_record`]):
//!   write a run as a JSON [`report::RunRecord`] and regenerate `report.html`. Driven automatically when
//!   a [`RunMeta`] carries a [`persist::Persist`] target (see [`RunMeta::persist_to`]).
//! - [`case`] — the generic, RON-authored case container [`EvalCase`] + the fail-loud [`load_cases`]
//!   loader (and [`parse_cases_from_str`] for one-or-many cases per file), both generic over the host's
//!   `Setup`/`Expect` types.
//! - [`baseline`](mod@baseline) — a shipped, ready-to-run baseline capability suite
//!   ([`baseline()`](baseline()) / [`baseline_files`]): basic arithmetic / language / tool-use checks,
//!   embedded into the crate, that a user runs against their agent in one call or copies as a template.
//! - [`harness`] — the [`Harness`] trait (the thing being benchmarked), the easy-path [`Agent`] trait,
//!   [`RunArtifacts`] (what one run produced, minus scoring), and the structured [`harness::ToolCall`].
//! - [`expect`] — the built-in assertion library [`expect::Expectation`] (tool use / text / math /
//!   health checks over [`RunArtifacts`]), serde/RON-authored.
//! - [`scorer`] — the [`Scorer`] trait (score one expectation against the post-run world + artifacts)
//!   and the batteries-included [`BuiltinScorer`].
//! - [`runner`] — the generic engine: [`run_eval`] / [`run_eval_with_meta`] tie a [`Harness`] + a
//!   [`Scorer`] over a shared world; [`run_suite`] / [`run_suite_with_meta`] are the easy path
//!   ([`Agent`] + [`BuiltinScorer`]). Both run every case, time each, isolate panics, and assemble an
//!   [`report::EvalReport`].
//! - [`error`] — the public [`EvalError`] surfaced by [`load_cases`] and [`Agent::run`].
//!
//! ## Advanced — the full path (custom world)
//!
//! When scoring needs post-run WORLD state, implement [`Harness`] over your agent + world, implement
//! [`Scorer`] over the same world, and call [`run_eval`]. See `examples/minimal.rs`.

pub mod baseline;
pub mod case;
pub mod error;
pub mod expect;
pub mod harness;
pub mod persist;
pub mod report;
pub mod report_html;
pub mod runner;
pub mod scorer;

pub use baseline::{baseline, baseline_files};
pub use case::{EvalCase, load_cases, parse_cases_from_str};
pub use error::EvalError;
pub use expect::Expectation;
pub use harness::{Agent, Harness, RunArtifacts, ToolCall};
pub use persist::{Persist, save_record};
pub use runner::{
    AgentHarness, RunMeta, run_eval, run_eval_with_meta, run_suite, run_suite_with_meta,
};
pub use scorer::{BuiltinScorer, Scorer};
