//! The built-in **assertion library**: [`Expectation`], a serde/RON-authored predicate over a run's
//! universal [`RunArtifacts`].
//!
//! This is what makes `eval-core` "pytest for agents": the common case needs NO user-implemented
//! [`Scorer`](crate::Scorer). A test case is a prompt plus a list of `Expectation`s asserting what the
//! agent DID — which tools it called, with which parameters, and what it finally said or computed —
//! scored by [`BuiltinScorer`](crate::BuiltinScorer) over the artifacts every harness already returns.
//!
//! ## RON authoring
//!
//! Variants read cleanly as RON (the enum is `#[derive(Deserialize)]` with struct-style variants):
//!
//! ```ron
//! (
//!   name: "adds two numbers",
//!   instruction: "what is 2 + 2?",
//!   expect: [
//!     CalledToolWith(tool: "calculator", args: { "op": "add", "a": 2, "b": 2 }),
//!     FinalNumberEquals(value: 4.0),
//!   ],
//! )
//! ```
//!
//! Note the case has NO `setup` field — it defaults to `()` for the easy [`Agent`](crate::Agent) path.
//!
//! ## Two matching rules to know
//!
//! - **Args subset match** ([`CalledToolWith`](Expectation::CalledToolWith)): the expected `args` JSON
//!   must be a SUBSET of the actual call's args — objects recurse key-by-key, every other JSON value
//!   (string/number/bool/null/array) must match exactly. So `{ "op": "add" }` matches a call made with
//!   `{ "op": "add", "a": 2, "b": 2 }`, but `{ "op": "sub" }` does not. (Arrays are compared whole, not
//!   element-subset.)
//! - **Number extraction** ([`FinalNumberEquals`](Expectation::FinalNumberEquals)): the *last* number in
//!   `final_text` is taken as the agent's answer (models typically end with the answer), then compared
//!   to `value` within `tolerance` (default `0.0` = exact). "Number" = an optionally-signed integer or
//!   decimal, with optional thousands separators stripped.

use serde::Deserialize;
use serde_json::Value;

use crate::error::EvalError;
use crate::harness::{RunArtifacts, ToolCall};

/// A single built-in assertion over a run's [`RunArtifacts`].
///
/// A case PASSES iff the run did not error AND every `Expectation` holds. Evaluate one against a run via
/// [`Expectation::evaluate`] (or let [`BuiltinScorer`](crate::BuiltinScorer) +
/// [`run_suite`](crate::run_suite) do it). Each variant produces a clear, report-ready label via
/// [`Expectation::label`].
#[derive(Debug, Clone, Deserialize)]
pub enum Expectation {
    // --- Tool use -------------------------------------------------------------------------------
    /// The agent called `tool` at least once (any args).
    CalledTool {
        /// The tool/function name that must appear among the calls.
        tool: String,
    },
    /// The agent did NOT call `tool` at all.
    DidNotCallTool {
        /// The tool/function name that must be absent from the calls.
        tool: String,
    },
    /// The agent called `tool` at least once with args that SUPERSET the given `args` (subset match —
    /// see the module docs). The canonical "called X with these parameters" assertion.
    CalledToolWith {
        /// The tool/function name that must have been called.
        tool: String,
        /// The expected argument subset (objects recurse; other values match exactly).
        args: Value,
    },
    /// The number of tool calls is within `[min, max]` (each optional). When `tool` is `Some`, only
    /// calls to that tool are counted; when `None`, ALL calls are counted.
    ToolCallCount {
        /// Restrict the count to this tool; `None` counts every call.
        #[serde(default)]
        tool: Option<String>,
        /// Inclusive lower bound on the count (optional).
        #[serde(default)]
        min: Option<usize>,
        /// Inclusive upper bound on the count (optional).
        #[serde(default)]
        max: Option<usize>,
    },
    /// The named tools appear as a SUBSEQUENCE of the call order (in order, but not necessarily
    /// contiguous — other calls may be interleaved). Empty `tools` trivially holds.
    CalledToolsInOrder {
        /// The tools that must appear in this relative order among the calls.
        tools: Vec<String>,
    },
    /// The agent made NO tool calls at all (a pure-reasoning / refusal assertion).
    NoToolCalls,

    // --- Text -----------------------------------------------------------------------------------
    /// `final_text` contains `text` (optionally case-insensitively). Fails when there is no final text.
    FinalTextContains {
        /// The substring that must be present in the final reply.
        text: String,
        /// Compare case-insensitively when `true` (default `false`, an exact-case substring match).
        #[serde(default)]
        case_insensitive: bool,
    },
    /// `final_text` equals `text` exactly (after trimming surrounding whitespace on both sides). Fails
    /// when there is no final text.
    FinalTextEquals {
        /// The exact (trimmed) final reply expected.
        text: String,
    },
    /// `final_text` matches the `regex` (anywhere, via [`regex::Regex::is_match`]). A malformed regex
    /// is a hard [`EvalError::Regex`] from [`Expectation::evaluate`] (NOT a silent failure). Fails when
    /// there is no final text.
    FinalTextMatches {
        /// The regular expression to match against the final reply.
        regex: String,
    },

    // --- Math -----------------------------------------------------------------------------------
    /// The LAST number in `final_text` equals `value` within `tolerance` (see the module docs for the
    /// extraction rule). Fails when there is no final text or it contains no number.
    FinalNumberEquals {
        /// The expected numeric answer.
        value: f64,
        /// Allowed absolute difference (default `0.0` = exact match).
        #[serde(default)]
        tolerance: f64,
    },

    // --- Health ---------------------------------------------------------------------------------
    /// The run reported no error ([`RunArtifacts::error`] is `None`). The runner already fails a case on
    /// any run error, so this is mostly for an explicit, labeled "the run was clean" predicate.
    NoError,
}

impl Expectation {
    /// Evaluate this expectation against a run's `artifacts`, returning `(label, passed)`.
    ///
    /// The label identifies WHICH predicate it is (for the report's per-predicate diagnostics), matching
    /// the labeling style of a hand-rolled scorer. The ONLY fallible case is
    /// [`FinalTextMatches`](Expectation::FinalTextMatches) with a malformed regex, which returns
    /// [`EvalError::Regex`]; every other variant is infallible.
    pub fn evaluate(&self, artifacts: &RunArtifacts) -> Result<(String, bool), EvalError> {
        let label = self.label();
        let passed = match self {
            Expectation::CalledTool { tool } => {
                calls_to(&artifacts.tool_calls, tool).next().is_some()
            }
            Expectation::DidNotCallTool { tool } => {
                calls_to(&artifacts.tool_calls, tool).next().is_none()
            }
            Expectation::CalledToolWith { tool, args } => calls_to(&artifacts.tool_calls, tool)
                .any(|call| json_subset_matches(args, &call.args)),
            Expectation::ToolCallCount { tool, min, max } => {
                let count = match tool {
                    Some(name) => calls_to(&artifacts.tool_calls, name).count(),
                    None => artifacts.tool_calls.len(),
                };
                min.is_none_or(|lo| count >= lo) && max.is_none_or(|hi| count <= hi)
            }
            Expectation::CalledToolsInOrder { tools } => {
                is_subsequence(tools, &artifacts.tool_calls)
            }
            Expectation::NoToolCalls => artifacts.tool_calls.is_empty(),

            Expectation::FinalTextContains {
                text,
                case_insensitive,
            } => match &artifacts.final_text {
                Some(actual) if *case_insensitive => {
                    actual.to_lowercase().contains(&text.to_lowercase())
                }
                Some(actual) => actual.contains(text),
                None => false,
            },
            Expectation::FinalTextEquals { text } => artifacts
                .final_text
                .as_deref()
                .is_some_and(|actual| actual.trim() == text.trim()),
            Expectation::FinalTextMatches { regex } => {
                // A bad regex is a hard error (a malformed assertion, not a failed one).
                let re = regex::Regex::new(regex).map_err(|source| EvalError::Regex {
                    pattern: regex.clone(),
                    source,
                })?;
                artifacts
                    .final_text
                    .as_deref()
                    .is_some_and(|actual| re.is_match(actual))
            }

            Expectation::FinalNumberEquals { value, tolerance } => artifacts
                .final_text
                .as_deref()
                .and_then(last_number)
                .is_some_and(|n| (n - value).abs() <= *tolerance),

            Expectation::NoError => artifacts.error.is_none(),
        };
        Ok((label, passed))
    }

    /// A short, human-readable label identifying this expectation, for the report's per-predicate lines.
    pub fn label(&self) -> String {
        match self {
            Expectation::CalledTool { tool } => format!("CalledTool({tool})"),
            Expectation::DidNotCallTool { tool } => format!("DidNotCallTool({tool})"),
            Expectation::CalledToolWith { tool, args } => format!("CalledToolWith({tool}, {args})"),
            Expectation::ToolCallCount { tool, min, max } => format!(
                "ToolCallCount({}, >= {min:?}, <= {max:?})",
                tool.as_deref().unwrap_or("any")
            ),
            Expectation::CalledToolsInOrder { tools } => {
                format!("CalledToolsInOrder({})", tools.join(" -> "))
            }
            Expectation::NoToolCalls => "NoToolCalls".to_owned(),
            Expectation::FinalTextContains {
                text,
                case_insensitive,
            } => {
                if *case_insensitive {
                    format!("FinalTextContains({text:?}, case-insensitive)")
                } else {
                    format!("FinalTextContains({text:?})")
                }
            }
            Expectation::FinalTextEquals { text } => format!("FinalTextEquals({text:?})"),
            Expectation::FinalTextMatches { regex } => format!("FinalTextMatches({regex:?})"),
            Expectation::FinalNumberEquals { value, tolerance } => {
                if *tolerance == 0.0 {
                    format!("FinalNumberEquals({value})")
                } else {
                    format!("FinalNumberEquals({value} ± {tolerance})")
                }
            }
            Expectation::NoError => "NoError".to_owned(),
        }
    }
}

/// Iterator over the calls made to `tool` (by name), in call order.
fn calls_to<'a>(calls: &'a [ToolCall], tool: &'a str) -> impl Iterator<Item = &'a ToolCall> {
    calls.iter().filter(move |c| c.name == tool)
}

/// Are `tools` a subsequence of the call names in `calls` — present in this relative order, though not
/// necessarily contiguous? An empty `tools` trivially holds.
fn is_subsequence(tools: &[String], calls: &[ToolCall]) -> bool {
    let mut wanted = tools.iter();
    let mut current = wanted.next();
    for call in calls {
        if let Some(want) = current
            && call.name == *want
        {
            current = wanted.next();
        }
    }
    current.is_none()
}

/// Is `expected` a SUBSET of `actual`? Objects recurse key-by-key (every key in `expected` must be
/// present in `actual` with a subset-matching value); every other JSON value must equal exactly.
///
/// Arrays are compared WHOLE (not element-subset): `[1, 2]` matches only `[1, 2]`. This keeps the rule
/// predictable for positional args like a `[x, y, z]` coordinate, where partial match would be
/// surprising.
fn json_subset_matches(expected: &Value, actual: &Value) -> bool {
    match (expected, actual) {
        (Value::Object(exp), Value::Object(act)) => exp.iter().all(|(k, exp_v)| {
            act.get(k)
                .is_some_and(|act_v| json_subset_matches(exp_v, act_v))
        }),
        // Exact match for scalars and arrays alike.
        _ => expected == actual,
    }
}

/// Extract the LAST number from `text` and return it as `f64`, or `None` if there is none.
///
/// "Number" = an optionally-signed run of digits with an optional single decimal point, with ASCII
/// thousands-separator commas tolerated inside the integer part (`1,024` → `1024`). The LAST such token
/// is returned because models typically end with the answer. A lone `-`/`.` (no digits) is not a number.
fn last_number(text: &str) -> Option<f64> {
    let bytes = text.as_bytes();
    let mut last: Option<f64> = None;
    let mut i = 0usize;
    while i < bytes.len() {
        // A number token may start with a sign, a digit, or a leading decimal point.
        let start = i;
        let mut j = i;
        // Optional leading sign.
        if j < bytes.len() && (bytes[j] == b'-' || bytes[j] == b'+') {
            j += 1;
        }
        let digits_start = j;
        let mut saw_digit = false;
        let mut saw_dot = false;
        while j < bytes.len() {
            match bytes[j] {
                b'0'..=b'9' => {
                    saw_digit = true;
                    j += 1;
                }
                // A comma is a thousands separator only between digits; otherwise it ends the token.
                b',' if !saw_dot
                    && saw_digit
                    && j + 1 < bytes.len()
                    && bytes[j + 1].is_ascii_digit() =>
                {
                    j += 1;
                }
                b'.' if !saw_dot => {
                    saw_dot = true;
                    j += 1;
                }
                _ => break,
            }
        }
        if saw_digit && j > digits_start {
            // Parse the token, stripping thousands-separator commas first.
            let token: String = text[start..j].chars().filter(|&c| c != ',').collect();
            if let Ok(n) = token.parse::<f64>() {
                last = Some(n);
            }
            i = j;
        } else {
            // Not the start of a number; advance one byte (ASCII-safe; multibyte prose just advances).
            i += 1;
        }
    }
    last
}

#[cfg(test)]
mod tests {
    #![allow(clippy::approx_constant)] // number-extraction fixtures use realistic decimals (e.g. 3.14159)
    use super::*;
    use serde_json::json;

    fn artifacts(calls: Vec<ToolCall>, final_text: Option<&str>) -> RunArtifacts {
        RunArtifacts {
            tool_calls: calls,
            final_text: final_text.map(str::to_owned),
            ..RunArtifacts::default()
        }
    }

    fn pass(exp: &Expectation, art: &RunArtifacts) -> bool {
        exp.evaluate(art).expect("infallible expectation").1
    }

    #[test]
    fn called_tool_and_did_not_call_tool() {
        let art = artifacts(
            vec![ToolCall::new("calculator", json!({"op": "add"}))],
            None,
        );
        assert!(pass(
            &Expectation::CalledTool {
                tool: "calculator".into()
            },
            &art
        ));
        assert!(!pass(
            &Expectation::CalledTool {
                tool: "search".into()
            },
            &art
        ));
        assert!(pass(
            &Expectation::DidNotCallTool {
                tool: "search".into()
            },
            &art
        ));
        assert!(!pass(
            &Expectation::DidNotCallTool {
                tool: "calculator".into()
            },
            &art
        ));
    }

    #[test]
    fn called_tool_with_subset_match() {
        let art = artifacts(
            vec![ToolCall::new(
                "calculator",
                json!({"op": "add", "a": 2, "b": 2}),
            )],
            None,
        );
        // Subset of the args → matches.
        assert!(pass(
            &Expectation::CalledToolWith {
                tool: "calculator".into(),
                args: json!({"op": "add"}),
            },
            &art
        ));
        // Wrong leaf value → no match.
        assert!(!pass(
            &Expectation::CalledToolWith {
                tool: "calculator".into(),
                args: json!({"op": "sub"}),
            },
            &art
        ));
        // Extra key the call didn't have → no match.
        assert!(!pass(
            &Expectation::CalledToolWith {
                tool: "calculator".into(),
                args: json!({"op": "add", "c": 9}),
            },
            &art
        ));
    }

    #[test]
    fn nested_subset_and_whole_array_match() {
        let art = artifacts(
            vec![ToolCall::new(
                "set_voxel",
                json!({"at": [1, 2, 3], "block": {"type": "stone", "hardness": 5}}),
            )],
            None,
        );
        // Nested object subset matches; array must match whole.
        assert!(pass(
            &Expectation::CalledToolWith {
                tool: "set_voxel".into(),
                args: json!({"block": {"type": "stone"}, "at": [1, 2, 3]}),
            },
            &art
        ));
        // A partial array does NOT match (arrays compared whole).
        assert!(!pass(
            &Expectation::CalledToolWith {
                tool: "set_voxel".into(),
                args: json!({"at": [1, 2]}),
            },
            &art
        ));
    }

    #[test]
    fn tool_call_count_bounds() {
        let art = artifacts(
            vec![
                ToolCall::new("a", json!({})),
                ToolCall::new("a", json!({})),
                ToolCall::new("b", json!({})),
            ],
            None,
        );
        // Total count in [3,3].
        assert!(pass(
            &Expectation::ToolCallCount {
                tool: None,
                min: Some(3),
                max: Some(3)
            },
            &art
        ));
        // Per-tool count for "a" is 2.
        assert!(pass(
            &Expectation::ToolCallCount {
                tool: Some("a".into()),
                min: Some(2),
                max: Some(2)
            },
            &art
        ));
        // Too few of "b".
        assert!(!pass(
            &Expectation::ToolCallCount {
                tool: Some("b".into()),
                min: Some(2),
                max: None
            },
            &art
        ));
    }

    #[test]
    fn called_tools_in_order_is_a_subsequence() {
        let art = artifacts(
            vec![
                ToolCall::new("plan", json!({})),
                ToolCall::new("search", json!({})),
                ToolCall::new("write", json!({})),
            ],
            None,
        );
        // Subsequence (non-contiguous) holds.
        assert!(pass(
            &Expectation::CalledToolsInOrder {
                tools: vec!["plan".into(), "write".into()]
            },
            &art
        ));
        // Wrong order fails.
        assert!(!pass(
            &Expectation::CalledToolsInOrder {
                tools: vec!["write".into(), "plan".into()]
            },
            &art
        ));
        // Empty trivially holds.
        assert!(pass(
            &Expectation::CalledToolsInOrder { tools: vec![] },
            &art
        ));
    }

    #[test]
    fn no_tool_calls() {
        assert!(pass(
            &Expectation::NoToolCalls,
            &artifacts(vec![], Some("hi"))
        ));
        assert!(!pass(
            &Expectation::NoToolCalls,
            &artifacts(vec![ToolCall::new("x", json!({}))], None)
        ));
    }

    #[test]
    fn final_text_contains_equals_matches() {
        let art = artifacts(vec![], Some("The answer is Forty-Two."));
        assert!(pass(
            &Expectation::FinalTextContains {
                text: "Forty-Two".into(),
                case_insensitive: false
            },
            &art
        ));
        // Case-sensitive miss, then case-insensitive hit.
        assert!(!pass(
            &Expectation::FinalTextContains {
                text: "forty-two".into(),
                case_insensitive: false
            },
            &art
        ));
        assert!(pass(
            &Expectation::FinalTextContains {
                text: "forty-two".into(),
                case_insensitive: true
            },
            &art
        ));
        // Equals trims surrounding whitespace.
        let trimmed = artifacts(vec![], Some("  done  "));
        assert!(pass(
            &Expectation::FinalTextEquals {
                text: "done".into()
            },
            &trimmed
        ));
        // Regex.
        assert!(pass(
            &Expectation::FinalTextMatches {
                regex: r"answer is \w+-\w+".into()
            },
            &art
        ));
    }

    #[test]
    fn final_text_matches_bad_regex_is_an_error() {
        let art = artifacts(vec![], Some("x"));
        let err = Expectation::FinalTextMatches { regex: "(".into() }
            .evaluate(&art)
            .expect_err("a malformed regex is a hard error");
        assert!(matches!(err, EvalError::Regex { .. }));
    }

    #[test]
    fn final_number_extraction_takes_the_last_number() {
        // The last number is the answer even with earlier numbers in the text.
        let art = artifacts(vec![], Some("Adding 2 and 2 gives 4"));
        assert!(pass(
            &Expectation::FinalNumberEquals {
                value: 4.0,
                tolerance: 0.0
            },
            &art
        ));
        // Tolerance.
        let art = artifacts(vec![], Some("approximately 3.1399"));
        assert!(pass(
            &Expectation::FinalNumberEquals {
                value: 3.14159,
                tolerance: 0.01
            },
            &art
        ));
        assert!(!pass(
            &Expectation::FinalNumberEquals {
                value: 3.14159,
                tolerance: 0.0001
            },
            &art
        ));
        // Negative + thousands separators.
        let art = artifacts(vec![], Some("the balance is -1,024.50"));
        assert!(pass(
            &Expectation::FinalNumberEquals {
                value: -1024.5,
                tolerance: 0.0
            },
            &art
        ));
        // No number → fails (rather than panicking).
        let art = artifacts(vec![], Some("no digits here"));
        assert!(!pass(
            &Expectation::FinalNumberEquals {
                value: 1.0,
                tolerance: 0.0
            },
            &art
        ));
    }

    #[test]
    fn no_error_reflects_artifacts_error() {
        let mut art = artifacts(vec![], None);
        assert!(pass(&Expectation::NoError, &art));
        art.error = Some("boom".into());
        assert!(!pass(&Expectation::NoError, &art));
    }

    /// Expectations deserialize from the RON authoring form shown in the module docs.
    #[test]
    fn expectations_deserialize_from_ron() {
        let exps: Vec<Expectation> = ron::from_str(
            r#"[
                CalledToolWith(tool: "calculator", args: { "op": "add", "a": 2, "b": 2 }),
                FinalNumberEquals(value: 4.0),
                ToolCallCount(tool: Some("calculator"), max: Some(1)),
                FinalTextContains(text: "four", case_insensitive: true),
                NoToolCalls,
            ]"#,
        )
        .expect("RON parses");
        assert_eq!(exps.len(), 5);
        assert!(matches!(exps[4], Expectation::NoToolCalls));
    }
}
