//! Scoring one expectation against a run's result, behind the [`Scorer`] trait — plus the
//! batteries-included [`BuiltinScorer`] that needs NO host scoring code at all.
//!
//! The host implements [`Scorer`] over the SAME `World` its [`Harness`](crate::Harness) produces. For
//! each of a case's `expect` predicates the runner calls [`Scorer::score`], collecting the returned
//! `(label, passed)` pairs into the case's [`CaseOutcome::predicates`](crate::report::CaseOutcome). A
//! case passes iff the run succeeded AND every predicate's `passed` is `true`.
//!
//! `score` now also receives the run's [`RunArtifacts`], so a scorer can assert on
//! what the agent DID (tool calls, params, final text) and not only on the post-run world. The common
//! case — "score the built-in [`Expectation`]s over the artifacts, ignoring
//! the world" — is [`BuiltinScorer`], which a host gets for free via [`run_suite`](crate::run_suite).

use crate::expect::Expectation;
use crate::harness::RunArtifacts;

/// Scores one expectation against the world AND artifacts a case produced. The host implements this for a
/// custom predicate/world; for the built-in assertions use [`BuiltinScorer`].
///
/// `score` returns BOTH a human-readable label and the pass/fail in one call (the existing AetherCore
/// scorer kept these as a paired `score()`/`label()`; folding them avoids re-deriving the label and
/// guarantees the label always matches the verdict). The label is shown in the report's per-predicate
/// diagnostics, so make it identify WHICH predicate it is (e.g. `"SolidPlaced(>= 4)"`).
pub trait Scorer {
    /// The world type to score against — must be the same world the paired
    /// [`Harness::World`](crate::Harness::World) produces (the runner enforces
    /// `Scorer<World = Harness::World>`).
    type World;
    /// The host's predicate type — matches the element type of [`EvalCase::expect`](crate::EvalCase::expect).
    type Expect;

    /// Score one `expect` predicate against the post-run `world` and the run's `artifacts`.
    ///
    /// Returns `(label, passed)`: a human-readable label for per-predicate diagnostics, and whether the
    /// predicate held. Must be side-effect-free with respect to `world` (it takes a shared reference);
    /// the runner may call it for several predicates over the same world. A scorer that only inspects
    /// the world can ignore `artifacts` (and vice versa).
    fn score(
        &self,
        expect: &Self::Expect,
        artifacts: &RunArtifacts,
        world: &Self::World,
    ) -> (String, bool);
}

/// The batteries-included scorer: evaluates the built-in [`Expectation`](crate::expect) assertions over
/// a run's [`RunArtifacts`], ignoring the world entirely (`World = ()`).
///
/// This is what removes the "implement a `Scorer`" step for the common case. Paired with the
/// [`Agent`](crate::Agent) trait + [`run_suite`](crate::run_suite), a host scores tool-call / parameter /
/// final-text assertions with zero scoring code of its own.
///
/// A malformed regex in a [`FinalTextMatches`](crate::expect::Expectation::FinalTextMatches) expectation
/// can't surface a `Result` through the infallible [`Scorer::score`] signature, so it is reported as a
/// FAILED predicate whose label names the regex error (rather than panicking or being silently dropped).
#[derive(Debug, Default, Clone, Copy)]
pub struct BuiltinScorer;

impl Scorer for BuiltinScorer {
    type World = ();
    type Expect = Expectation;

    fn score(&self, expect: &Expectation, artifacts: &RunArtifacts, _world: &()) -> (String, bool) {
        match expect.evaluate(artifacts) {
            Ok(result) => result,
            // A bad regex is an authoring error; surface it as a clearly-labeled failed predicate so the
            // report shows WHICH expectation is malformed instead of hiding it behind the infallible API.
            Err(err) => (format!("{} [invalid: {err}]", expect.label()), false),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::ToolCall;
    use serde_json::json;

    fn artifacts() -> RunArtifacts {
        RunArtifacts {
            tool_calls: vec![ToolCall::new(
                "calculator",
                json!({"op": "add", "a": 2, "b": 2}),
            )],
            final_text: Some("The answer is 4".to_owned()),
            ..RunArtifacts::default()
        }
    }

    #[test]
    fn builtin_scorer_evaluates_over_artifacts() {
        let art = artifacts();
        let (label, passed) = BuiltinScorer.score(
            &Expectation::CalledToolWith {
                tool: "calculator".into(),
                args: json!({"op": "add"}),
            },
            &art,
            &(),
        );
        assert!(passed);
        assert!(label.starts_with("CalledToolWith("));

        let (_, passed) = BuiltinScorer.score(
            &Expectation::FinalNumberEquals {
                value: 4.0,
                tolerance: 0.0,
            },
            &art,
            &(),
        );
        assert!(passed);
    }

    #[test]
    fn builtin_scorer_reports_bad_regex_as_failed_labeled_predicate() {
        let (label, passed) = BuiltinScorer.score(
            &Expectation::FinalTextMatches { regex: "(".into() },
            &artifacts(),
            &(),
        );
        assert!(!passed, "a malformed regex scores as a failed predicate");
        assert!(
            label.contains("invalid:"),
            "label names the regex error: {label}"
        );
    }
}
