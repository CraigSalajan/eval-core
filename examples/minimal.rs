//! The canonical, dependency-free "how to use `eval-core`" example.
//!
//! It defines a trivial domain with ZERO game/LLM dependencies:
//! - `World`  = a list of "actions" the harness performed (`Vec<String>`).
//! - `Setup`  = an initial seed action the world starts with.
//! - `Expect` = "the world contains this string".
//!
//! Then it implements [`Harness`] (whose `run` just records a couple of actions) and [`Scorer`],
//! constructs a couple of [`EvalCase`]s, runs them through [`run_eval`], and asserts the resulting
//! [`EvalReport`] has the expected pass/fail counts. Run with: `cargo run -p eval-core --example minimal`.

use eval_core::{EvalCase, Harness, RunArtifacts, Scorer, ToolCall, run_eval};

/// The world a case runs against: an ordered log of actions performed.
#[derive(Debug, Default)]
struct World {
    actions: Vec<String>,
}

/// How to build the world for a case: it starts seeded with one action.
#[derive(Debug, Default, serde::Deserialize)]
struct Setup {
    /// An action the world is pre-seeded with (so a case can stage starting state).
    seed: String,
}

/// One predicate: the world's action log must contain `contains`.
#[derive(Debug, serde::Deserialize)]
struct Expect {
    /// The substring/action that must be present for the predicate to hold.
    contains: String,
}

/// A trivial harness: its `run` records the instruction plus a fixed follow-up action, and reports a
/// made-up token count so the report's token stats are exercised too.
struct DemoHarness;

impl Harness for DemoHarness {
    type World = World;
    type Setup = Setup;

    fn setup(&self, setup: &Self::Setup) -> Self::World {
        World {
            actions: vec![setup.seed.clone()],
        }
    }

    fn run(&self, instruction: &str, world: &mut Self::World) -> anyhow::Result<RunArtifacts> {
        // "Execute" the instruction: record it, then perform a canned follow-up action.
        world.actions.push(format!("did:{instruction}"));
        world.actions.push("cleanup".to_owned());

        Ok(RunArtifacts::new()
            .with_tool_calls(vec![
                ToolCall::new("perform", serde_json::json!({ "instruction": instruction })),
                ToolCall::new("cleanup", serde_json::json!({})),
            ])
            .with_final_text(format!("done: {instruction}"))
            .with_tokens(42))
    }
}

/// A trivial scorer: a predicate passes iff some recorded action contains its `contains` string.
struct DemoScorer;

impl Scorer for DemoScorer {
    type World = World;
    type Expect = Expect;

    fn score(
        &self,
        expect: &Self::Expect,
        _artifacts: &RunArtifacts,
        world: &Self::World,
    ) -> (String, bool) {
        let passed = world.actions.iter().any(|a| a.contains(&expect.contains));
        (format!("contains({:?})", expect.contains), passed)
    }
}

fn expect(contains: &str) -> Expect {
    Expect {
        contains: contains.to_owned(),
    }
}

fn main() {
    let cases: Vec<EvalCase<Setup, Expect>> = vec![
        // PASSES: both predicates hold — the harness records "did:build a wall" and the seed "ready".
        EvalCase {
            name: "passing".to_owned(),
            instruction: "build a wall".to_owned(),
            setup: Setup {
                seed: "ready".to_owned(),
            },
            expect: vec![expect("did:build a wall"), expect("ready")],
        },
        // FAILS: the second predicate looks for an action the harness never records.
        EvalCase {
            name: "failing".to_owned(),
            instruction: "dig a hole".to_owned(),
            setup: Setup::default(),
            expect: vec![expect("did:dig a hole"), expect("teleport")],
        },
    ];

    let report = run_eval(&DemoHarness, &DemoScorer, &cases);

    // Print the human-readable summary table (stdout stays clean of progress — that went to stderr).
    println!("{report}");

    // Correctness assertions: exactly one of the two cases passed.
    assert_eq!(report.total(), 2, "ran both cases");
    assert_eq!(report.passed(), 1, "exactly one case passed");

    let passing = &report.outcomes[0];
    assert!(passing.passed, "the first case should pass");
    assert_eq!(passing.predicates.len(), 2);
    assert!(passing.predicates.iter().all(|(_, p)| *p));
    assert_eq!(
        passing.tokens,
        Some(42),
        "harness-reported token count flows through"
    );
    assert_eq!(passing.tool_calls.len(), 2);

    let failing = &report.outcomes[1];
    assert!(!failing.passed, "the second case should fail");
    // The first predicate held; only the second ("teleport") failed.
    assert!(failing.predicates[0].1, "did:dig a hole was recorded");
    assert!(!failing.predicates[1].1, "teleport was never recorded");

    println!("\nminimal example OK: 1 passed, 1 failed as expected");
}
