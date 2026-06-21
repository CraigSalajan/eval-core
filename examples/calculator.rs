//! The canonical "how to use the agent testing framework" example: a fake calculator agent (NO real
//! LLM) tested with the built-in [`Expectation`]s — no `World`, no `Setup`, no `Scorer` impl.
//!
//! It shows the whole easy path:
//! 1. implement ONE method, [`Agent::run`], over your harness;
//! 2. author `EvalCase<(), Expectation>` cases (here, inline; in practice often RON via
//!    [`eval_core::load_cases`]);
//! 3. call [`run_suite`] and read the [`EvalReport`].
//!
//! Run with: `cargo run -p eval-core --example calculator`.
//!
//! ## Optional: upload results to EvalForge
//!
//! Set `EVALFORGE_API_KEY` + `EVALFORGE_PROJECT_ID` to also POST the run to the EvalForge dashboard
//! (evalforge.ai) after it finishes:
//!
//! ```sh
//! EVALFORGE_API_KEY=sk-eval-... EVALFORGE_PROJECT_ID=<project-uuid> \
//!   cargo run -p eval-core --example calculator
//! ```
//!
//! With neither env var present, the example behaves exactly as before (it runs the suite offline and
//! uploads nothing).

use eval_core::{Agent, EvalCase, EvalError, Expectation, RunArtifacts, ToolCall, run_suite};
use serde_json::json;

/// A toy "calculator agent": it parses an `"<a> <op> <b>"` arithmetic instruction, emits a structured
/// `calculator` tool call, and returns the result as its final text. For a non-arithmetic instruction it
/// makes NO tool call and just greets — so the test suite below can assert both shapes. No LLM involved.
struct CalculatorAgent;

impl CalculatorAgent {
    /// Try to read `"<a> <op> <b>"` (e.g. `"2 + 2"` or `"what is 2 + 2?"`) out of `instruction`,
    /// returning `(op_name, a, b, result)`. `op_name` is the symbolic operator mapped to a word
    /// (`add`/`sub`/…). Robust to surrounding text and punctuation — extracts the last number
    /// before the operator and the first number after.
    fn parse_arithmetic(instruction: &str) -> Option<(&'static str, f64, f64, f64)> {
        /// Collect every contiguous number (integers or decimals) in `s`, left to right.
        fn numbers_in(s: &str) -> Vec<f64> {
            let mut nums = Vec::new();
            let mut cur = String::new();
            for ch in s.chars() {
                if ch.is_ascii_digit() || (ch == '.' && !cur.contains('.')) {
                    cur.push(ch);
                } else if !cur.is_empty() {
                    if let Ok(n) = cur.parse::<f64>() {
                        nums.push(n);
                    }
                    cur.clear();
                }
            }
            if let Ok(n) = cur.parse::<f64>() {
                nums.push(n); // trailing number with no separator after it
            }
            nums
        }

        /// First number in the string (digits kept in their original order).
        fn extract_first_number(s: &str) -> Option<f64> {
            numbers_in(s).into_iter().next()
        }

        /// Last number in the string (e.g. the `10` in `"compute 10 / 3"`).
        fn extract_last_number(s: &str) -> Option<f64> {
            numbers_in(s).into_iter().last()
        }

        // Find the operator token and split around it (very forgiving — this is a stand-in for an LLM).
        for (sym, name) in [("+", "add"), ("-", "sub"), ("*", "mul"), ("/", "div")] {
            if let Some((lhs, rhs)) = instruction.split_once(sym) {
                let a = extract_last_number(lhs)?;
                let b = extract_first_number(rhs)?;
                let result = match name {
                    "add" => a + b,
                    "sub" => a - b,
                    "mul" => a * b,
                    _ => a / b,
                };
                return Some((name, a, b, result));
            }
        }
        None
    }
}

impl Agent for CalculatorAgent {
    fn run(&self, instruction: &str) -> Result<RunArtifacts, EvalError> {
        match Self::parse_arithmetic(instruction) {
            // Arithmetic: emit a structured tool call + answer in the final text.
            Some((op, a, b, result)) => Ok(RunArtifacts::new()
                .with_tool_calls(vec![ToolCall::new(
                    "calculator",
                    json!({ "op": op, "a": a, "b": b }),
                )])
                .with_final_text(format!("The answer is {result}."))),
            // Non-arithmetic: no tool call, just a greeting.
            None => Ok(RunArtifacts::new().with_final_text(format!(
                "Hello! I can do math, but I can't help with: {instruction}"
            ))),
        }
    }
}

fn main() {
    let cases: Vec<EvalCase<(), Expectation>> = vec![
        // PASSES: calls the calculator with op=add and the final number is 4.
        EvalCase {
            name: "adds-two-numbers".to_owned(),
            instruction: "what is 2 + 2?".to_owned(),
            setup: (),
            expect: vec![
                Expectation::CalledToolWith {
                    tool: "calculator".to_owned(),
                    args: json!({ "op": "add" }),
                },
                Expectation::FinalNumberEquals {
                    value: 4.0,
                    tolerance: 0.0,
                },
            ],
        },
        // PASSES: division within a tolerance + a case-insensitive text check.
        EvalCase {
            name: "divides-with-tolerance".to_owned(),
            instruction: "compute 10 / 3".to_owned(),
            setup: (),
            expect: vec![
                Expectation::CalledTool {
                    tool: "calculator".to_owned(),
                },
                Expectation::FinalNumberEquals {
                    value: 3.333,
                    tolerance: 0.01,
                },
                Expectation::FinalTextContains {
                    text: "the answer".to_owned(),
                    case_insensitive: true,
                },
            ],
        },
        // PASSES: a non-arithmetic prompt makes NO tool call.
        EvalCase {
            name: "no-tools-for-chitchat".to_owned(),
            instruction: "hello there".to_owned(),
            setup: (),
            expect: vec![Expectation::NoToolCalls],
        },
        // FAILS: the agent does add (op=add), so asserting op=sub must fail.
        EvalCase {
            name: "wrong-op-expectation-fails".to_owned(),
            instruction: "what is 2 + 2?".to_owned(),
            setup: (),
            expect: vec![Expectation::CalledToolWith {
                tool: "calculator".to_owned(),
                args: json!({ "op": "sub" }),
            }],
        },
    ];

    // When both `EVALFORGE_API_KEY` + `EVALFORGE_PROJECT_ID` are set, the run is also uploaded to
    // EvalForge; otherwise this is exactly `run_suite(&CalculatorAgent, &cases)`.
    let report = run_calculator_suite(&cases);

    // The human-readable summary table (progress went to stderr; stdout stays clean for the report).
    println!("{report}");

    // Three of the four cases pass; the deliberately-wrong assertion fails.
    assert_eq!(report.total(), 4, "ran all four cases");
    assert_eq!(report.passed(), 3, "exactly three cases pass");
    assert!(report.outcomes[0].passed, "adds-two-numbers passes");
    assert!(report.outcomes[1].passed, "divides-with-tolerance passes");
    assert!(report.outcomes[2].passed, "no-tools-for-chitchat passes");
    assert!(!report.outcomes[3].passed, "wrong-op-expectation fails");

    // The structured tool call is rendered to a display string in the report.
    assert!(
        report.outcomes[0].tool_calls[0].starts_with("calculator("),
        "tool call display string derived from the structured ToolCall: {:?}",
        report.outcomes[0].tool_calls
    );

    println!("\ncalculator example OK: 3 passed, 1 failed as expected");
}

/// Run the suite, opting into an EvalForge upload only when both `EVALFORGE_API_KEY` +
/// `EVALFORGE_PROJECT_ID` are present. This keeps the example runnable offline: with either env var
/// unset, it is a plain `run_suite`.
fn run_calculator_suite(cases: &[EvalCase<(), Expectation>]) -> eval_core::report::EvalReport {
    use eval_core::{RunMeta, run_suite_with_meta};

    match std::env::var("EVALFORGE_PROJECT_ID") {
        Ok(project_id) if !project_id.is_empty() => {
            // `upload_from_env` reads EVALFORGE_API_KEY and, if it is unset, just warns and skips upload.
            let meta = RunMeta::new(0.0, "example: calculator", "")
                .upload_from_env(project_id)
                .upload_model("calculator-example");
            run_suite_with_meta(&CalculatorAgent, cases, meta)
        }
        _ => run_suite(&CalculatorAgent, cases),
    }
}
