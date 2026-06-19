//! A shipped, ready-to-run **baseline capability suite** — basic checks any agent can be measured
//! against in one call.
//!
//! ```ignore
//! let report = eval_core::run_suite(&my_agent, &eval_core::baseline());
//! println!("{report}");
//! ```
//!
//! The suite covers three capability areas, authored as RON files grouped by capability and EMBEDDED
//! into this crate (so they ship with the published crate — nothing is read from disk at runtime):
//!
//! - `arithmetic.ron` — basic math (number assertions, plus a clearly-labelled opt-in tool-use subset),
//! - `language.ron` — instruction following over the agent's final text (fully portable),
//! - `tool_use.ron` — tool-calling with parameters (assumes a DOCUMENTED tool-name convention — adapt it).
//!
//! ## Portable vs. adapt-me
//!
//! The number/text assertions are PORTABLE: they inspect only the agent's final reply, so they hold
//! whether the agent uses tools or reasons inline. The tool-use cases (and a small subset of the
//! arithmetic file) additionally assert specific tool names/args, which a user must adapt to their own
//! agent — every such case is clearly commented at the top of its file.
//!
//! ## Use it, or fork it
//!
//! Run it directly via [`baseline`], OR dump the raw embedded RON via [`baseline_files`] as a starting
//! template you copy into your own suite directory and edit. The same files back both paths, so the
//! template you copy is exactly what [`baseline`] runs.

use include_dir::{Dir, include_dir};

use crate::EvalCase;
use crate::case::parse_cases_from_str;
use crate::expect::Expectation;

/// The baseline RON files, embedded at compile time so they ship inside the crate (not read from disk).
static BASELINE_DIR: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/baseline");

/// The ready-made baseline capability suite: every embedded case, parsed and concatenated.
///
/// Returns `EvalCase<(), Expectation>` cases (no `setup`; the easy [`Agent`](crate::Agent) path), ready
/// to hand straight to [`run_suite`](crate::run_suite):
///
/// ```ignore
/// let report = eval_core::run_suite(&my_agent, &eval_core::baseline());
/// ```
///
/// ## Why this is infallible at the call site
///
/// The embedded RON is authored and tested IN THIS CRATE, so a parse failure here would be OUR bug, not
/// the user's — and the `baseline_parses_and_is_well_formed` test parses exactly these files on every
/// `cargo test`, catching any malformed edit before release. Hence this returns the `Vec` directly and
/// `expect`s on parse, rather than burdening every caller with a `Result` they can't meaningfully handle.
/// Files load in sorted (filename) order, cases in authored order within each file.
///
/// To instead get the raw files as a copy-and-edit template, see [`baseline_files`].
pub fn baseline() -> Vec<EvalCase<(), Expectation>> {
    let mut cases = Vec::new();
    for (name, contents) in baseline_files() {
        // `expect`, not `Result`: the data is ours and a test parses it — a panic here is a build-time
        // contract violation a developer fixes, never something a downstream user hits at runtime.
        let parsed = parse_cases_from_str::<(), Expectation>(name, contents)
            .unwrap_or_else(|e| panic!("embedded baseline file `{name}` is malformed: {e}"));
        cases.extend(parsed);
    }
    cases
}

/// The raw embedded baseline files as `(file_name, ron_contents)` pairs, in sorted (filename) order.
///
/// Use this to DUMP the suite as a starting template — write each pair to your own cases directory, then
/// edit the instructions, values, and (in `tool_use.ron`) the tool names to match your agent:
///
/// ```ignore
/// for (name, contents) in eval_core::baseline_files() {
///     std::fs::write(my_suite_dir.join(name), contents)?;
/// }
/// ```
///
/// The same files back [`baseline`], so what you copy here is exactly what [`baseline`] runs.
pub fn baseline_files() -> &'static [(&'static str, &'static str)] {
    // Build the sorted slice ONCE into a process-lifetime `OnceLock`. `include_dir` iteration order is
    // unspecified, so we sort by file name to match `load_cases`' deterministic ordering. The pairs
    // borrow the compile-time-embedded `Dir<'static>`, so the stored data is `'static` with no leak.
    use std::sync::OnceLock;
    static FILES: OnceLock<Vec<(&'static str, &'static str)>> = OnceLock::new();
    FILES.get_or_init(|| {
        let mut files: Vec<(&'static str, &'static str)> = BASELINE_DIR
            .files()
            .filter(|f| {
                f.path()
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("ron"))
            })
            .filter_map(|f| {
                // File name (e.g. "arithmetic.ron") + its UTF-8 contents. Embedded RON is UTF-8.
                let name = f.path().file_name()?.to_str()?;
                let contents = f.contents_utf8()?;
                Some((name, contents))
            })
            .collect();
        files.sort_by_key(|(name, _)| *name);
        files
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// The embedded baseline parses, has the expected number of cases, and every case is well-formed
    /// (non-empty name + instruction, at least one expectation) with a unique name. This is the test
    /// that justifies [`baseline`]'s internal `expect`: a malformed embedded edit fails CI here.
    #[test]
    fn baseline_parses_and_is_well_formed() {
        let cases = baseline();

        // 6 arithmetic + 5 language + 7 tool-use = 18.
        assert_eq!(
            cases.len(),
            18,
            "expected 18 baseline cases, got {}",
            cases.len()
        );

        let mut names = HashSet::new();
        for case in &cases {
            assert!(
                !case.name.trim().is_empty(),
                "a baseline case has an empty name"
            );
            assert!(
                !case.instruction.trim().is_empty(),
                "baseline case `{}` has an empty instruction",
                case.name
            );
            assert!(
                !case.expect.is_empty(),
                "baseline case `{}` has no expectations",
                case.name
            );
            assert!(
                names.insert(case.name.clone()),
                "duplicate baseline case name `{}`",
                case.name
            );
        }
    }

    /// The three expected capability files are present in the embedded set, in sorted order.
    #[test]
    fn baseline_files_are_the_three_capability_files() {
        let names: Vec<&str> = baseline_files().iter().map(|(n, _)| *n).collect();
        assert_eq!(
            names,
            vec!["arithmetic.ron", "language.ron", "tool_use.ron"]
        );
    }
}
