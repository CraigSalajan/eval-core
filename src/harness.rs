//! The thing being benchmarked: the host's agent harness, behind the [`Harness`] trait, plus
//! [`RunArtifacts`] — everything a single run produced EXCEPT scoring — and the structured [`ToolCall`]
//! the artifacts carry.
//!
//! `eval-core` knows nothing about HOW a run happens (which LLM, which tools, which world). The host
//! implements [`Harness`] over its own `World` + `Setup`, and the generic runner ([`crate::run_eval`])
//! drives it: build a fresh world per case, run the instruction against it, then hand the resulting
//! world AND the run's [`RunArtifacts`] to a [`Scorer`](crate::Scorer). The harness owns its own backend
//! and reports its own token count — there is deliberately no backend/token-counting trait in this crate.
//!
//! For the common case — scoring what the agent DID (tool calls, params, final text/number) rather than
//! a custom world — a host can skip [`Harness`]/[`Scorer`](crate::Scorer)/`World`/`Setup` entirely and implement the
//! one-method [`Agent`] trait, authoring [`Expectation`](crate::expect::Expectation) predicates and
//! calling [`run_suite`](crate::run_suite). See that trait + the `calculator` example.

use serde_json::Value;

use crate::error::EvalError;

/// One tool call the agent made: the tool's `name` and the structured `args` it was invoked with.
///
/// `args` is opaque JSON so any argument shape round-trips and the built-in
/// [`expect::Expectation`](crate::expect) assertions can subset-match against it. The report keeps a
/// *display* string per call (see [`CaseOutcome::tool_calls`](crate::report::CaseOutcome::tool_calls));
/// the runner derives those from these structured calls via [`ToolCall::display`], so saved JSON / the
/// `--json` path / the HTML report stay shape-compatible.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub struct ToolCall {
    /// The tool/function name the agent invoked.
    pub name: String,
    /// The arguments the agent passed, as opaque JSON (typically an object).
    pub args: Value,
}

impl ToolCall {
    /// Build a tool call from a name and its argument JSON.
    pub fn new(name: impl Into<String>, args: Value) -> Self {
        Self {
            name: name.into(),
            args,
        }
    }

    /// Render the compact `"name(compact-args)"` form used in the report's per-case `tool_calls` display
    /// strings (and the human-readable summary table). The args are the JSON value's compact string, so
    /// `set_voxel({"at":[0,1,0]})` reads back the way a debugging caller expects.
    pub fn display(&self) -> String {
        format!("{}({})", self.name, self.args)
    }
}

impl std::fmt::Display for ToolCall {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}({})", self.name, self.args)
    }
}

/// Everything a single run produced EXCEPT scoring. The [`Harness`] (or [`Agent`]) fills this in and
/// returns it; the generic runner copies it onto the case's [`CaseOutcome`](crate::report::CaseOutcome)
/// and also hands it to the [`Scorer`](crate::Scorer) so built-in assertions can inspect what the agent
/// did.
///
/// Every field is optional/empty by default, so a minimal harness can return `RunArtifacts::default()`
/// and a richer one can populate as much diagnostic detail as it has.
///
/// `#[non_exhaustive]`: new diagnostic fields can be added without a breaking change. Within `eval-core`
/// it is still constructed with struct literals / `..Default::default()`; external crates build it via
/// the builder methods (e.g. `RunArtifacts::new().with_final_text(...)`).
#[derive(Debug, Default, Clone)]
#[non_exhaustive]
pub struct RunArtifacts {
    /// The agent's tool calls, each as a structured [`ToolCall`] (name + args). The runner derives the
    /// report's display strings from these via [`ToolCall::display`].
    pub tool_calls: Vec<ToolCall>,
    /// The agent's final free-text reply (e.g. a summary line), if any.
    pub final_text: Option<String>,
    /// The harness's own completion-token count for this run, when it has one. The harness owns its
    /// backend, so it reports this itself; the runner just forwards it. `None` means "not reported"
    /// (distinct from a reported `0`).
    pub tokens: Option<u32>,
    /// The full per-run message log, if the harness captured one (rendered by the HTML report's
    /// per-case expander). Opaque JSON so any transcript shape round-trips.
    pub transcript: Vec<Value>,
    /// A run-level error the harness captured WITHOUT failing the call (e.g. a recoverable backend
    /// hiccup it logged). A hard failure should instead be returned as `Err` from [`Harness::run`];
    /// either way the case is marked failed. When both are present, the runner prefers the `Err`.
    pub error: Option<String>,
}

impl RunArtifacts {
    /// An empty result; fill it in with the `with_*` builders.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the tool calls (chainable builder).
    pub fn with_tool_calls(mut self, tool_calls: Vec<ToolCall>) -> Self {
        self.tool_calls = tool_calls;
        self
    }

    /// Set the final text (chainable builder).
    pub fn with_final_text(mut self, text: impl Into<String>) -> Self {
        self.final_text = Some(text.into());
        self
    }

    /// Set the completion token count (chainable builder).
    pub fn with_tokens(mut self, tokens: u32) -> Self {
        self.tokens = Some(tokens);
        self
    }

    /// Set the transcript (chainable builder).
    pub fn with_transcript(mut self, transcript: Vec<Value>) -> Self {
        self.transcript = transcript;
        self
    }

    /// Set a run-level error (chainable builder).
    pub fn with_error(mut self, error: impl Into<String>) -> Self {
        self.error = Some(error.into());
        self
    }
}

/// The thing being benchmarked: the host's agent harness. The host implements this over its own world.
///
/// The runner calls [`setup`](Harness::setup) once per case to build a FRESH world, then
/// [`run`](Harness::run) once against that world. Implementations should be deterministic given the same
/// `Setup` (the eval forces a deterministic configuration where it can) so runs are comparable.
///
/// Method calls are isolated behind `catch_unwind` by the runner, so a panic inside `setup`/`run` fails
/// only the offending case; implementors need not catch their own panics.
///
/// For the common "score what the agent DID, against `()` world + the built-in
/// [`Expectation`](crate::expect::Expectation)s" case, prefer the simpler [`Agent`] trait +
/// [`run_suite`](crate::run_suite) — no `World`/`Setup`/`Scorer` to implement.
pub trait Harness {
    /// The mutable world a case runs against (e.g. an executor over a bare game world). Built by
    /// [`setup`](Harness::setup), mutated by [`run`](Harness::run), then scored by a
    /// [`Scorer`](crate::Scorer) whose `Scorer::World` must match this type.
    type World;
    /// The host's per-case setup description (the input that builds a [`World`](Harness::World)).
    /// Matches [`EvalCase::setup`](crate::EvalCase::setup).
    type Setup;

    /// Build a fresh world for one case from its `setup`. Called exactly once per case, immediately
    /// before [`run`](Harness::run). A panic here fails only this case.
    fn setup(&self, setup: &Self::Setup) -> Self::World;

    /// Run `instruction` against `world`, mutating it, and return what the run produced (tool calls,
    /// final text, tokens, transcript). Wall-clock latency is timed by the runner around this call.
    ///
    /// Return `Err` for a run-level failure (backend error, etc.); the case is then marked failed and
    /// the error recorded. Predicates are still scored against whatever world state exists, but a case
    /// can never PASS when `run` returned `Err`.
    fn run(&self, instruction: &str, world: &mut Self::World) -> anyhow::Result<RunArtifacts>;
}

/// The easy path: implement this single method for your harness — run ONE prompt, return what the agent
/// did — and the framework does the rest.
///
/// Pair an `Agent` with the built-in [`Expectation`](crate::expect::Expectation) assertions and call
/// [`run_suite`](crate::run_suite): no `World`, no `Setup`, no [`Scorer`](crate::Scorer) impl. Internally
/// an adapter ([`AgentHarness`](crate::AgentHarness)) wraps the agent as a `Harness<World = (), Setup =
/// ()>`, and [`BuiltinScorer`](crate::BuiltinScorer) scores the returned [`RunArtifacts`].
///
/// This is "pytest for agents": a test case is a prompt + [`Expectation`](crate::expect::Expectation)s,
/// scored over the universal [`RunArtifacts`] with no user-implemented scorer.
pub trait Agent {
    /// Run `instruction` once and return what the agent did ([`RunArtifacts`]: tool calls, final text,
    /// tokens, transcript). Return [`EvalError::agent`] for a run-level failure; the runner marks the
    /// case failed and records the error, but still scores every expectation over the artifacts.
    fn run(&self, instruction: &str) -> Result<RunArtifacts, EvalError>;
}
