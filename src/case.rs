//! The generic, RON-authored eval **case schema** + loader.
//!
//! An [`EvalCase`] is one natural-language instruction, the world it runs against (the host's `Setup`),
//! and the host's `Expect` predicates its result must satisfy. The case container is domain-agnostic:
//! `Setup` and `Expect` are the host's own types (e.g. AetherCore's voxel `EvalSetup` / `Expectation`),
//! so `eval-core` stays free of any game/LLM specifics.
//!
//! Cases are authored as `.ron` files and loaded by [`load_cases`], which is fail-loud (a malformed
//! file is a hard error naming it) and deterministic (files are read in sorted order).
//!
//! ## One file, one OR many cases
//!
//! A `.ron` case file may hold EITHER a single [`EvalCase`] or a list `[EvalCase, EvalCase, …]` of
//! related cases — see [`parse_cases_from_str`], which [`load_cases`] uses per file. This lets an author
//! group a whole capability (say all the arithmetic checks) in one file without one-file-per-case
//! sprawl, while existing single-case files keep loading unchanged.

use serde::Deserialize;
use serde::de::DeserializeOwned;

use crate::error::EvalError;

/// One eval case: a named instruction, the world it runs in (`Setup`), and the predicates
/// (`Expect`) its result must satisfy.
///
/// Generic over the host's two domain types:
/// - `Setup`: how to build the world the case runs against (the input to [`crate::Harness::setup`]).
/// - `Expect`: a single predicate scored against the post-run world (by [`crate::Scorer::score`]).
///
/// A case PASSES iff EVERY one of its `expect` predicates holds (and the run did not error); each
/// predicate's pass/fail is also recorded individually for debugging (see [`crate::report::CaseOutcome`]).
#[derive(Debug, Clone, Deserialize)]
pub struct EvalCase<Setup, Expect> {
    /// Stable case name (also used in the report). Conventionally matches the file stem.
    pub name: String,
    /// The natural-language instruction handed to the harness verbatim (the user turn).
    pub instruction: String,
    /// How to build the world the case runs against.
    ///
    /// `#[serde(default)]` lets a case omit `setup` entirely and get `Setup::default()` — so this
    /// requires `Setup: Default` at deserialization time (RON only; the in-memory struct has no such
    /// bound).
    #[serde(default)]
    pub setup: Setup,
    /// The predicates the result must satisfy (ALL must hold for the case to pass).
    pub expect: Vec<Expect>,
}

/// Parse one `.ron` case file's `contents` into a list of [`EvalCase`]s, accepting EITHER authoring shape.
///
/// `name` is used only for error messages (typically the file path/stem). The parse tries the LIST form
/// (`Vec<EvalCase>`, RON `[EvalCase(…), EvalCase(…)]`) FIRST, and only if that fails falls back to the
/// SINGLE form (one `EvalCase(…)`), wrapping the lone case in a one-element `Vec`. List-first is the
/// correct order: a single `EvalCase(…)` can never parse as a `Vec` (it doesn't start with `[`), so a
/// genuine single-case file flows cleanly to the fallback; whereas single-first would mis-handle a list.
///
/// ### Error reporting when BOTH parses fail
///
/// A file that is neither a valid list nor a valid single case is a hard [`EvalError::RonParse`] naming
/// `name`. We report the *single*-case parse error as the `source`: for a file the author intended as one
/// case (the common shape, and what a typo'd single-case file is) it is the directly relevant message,
/// and for a genuinely-malformed list the single-parse error is just as diagnostic (RON still points at
/// the offending span). We do not silently drop the case.
///
/// Bounds: `Setup: DeserializeOwned + Default` (a case may omit `setup`); `Expect: DeserializeOwned`.
pub fn parse_cases_from_str<Setup, Expect>(
    name: &str,
    contents: &str,
) -> Result<Vec<EvalCase<Setup, Expect>>, EvalError>
where
    Setup: DeserializeOwned + Default,
    Expect: DeserializeOwned,
{
    // LIST form first: `[EvalCase(…), …]`. A single `EvalCase(…)` can't parse as a Vec, so this
    // cleanly fails over to the single-case branch below for genuine one-case files.
    if let Ok(list) = ron::from_str::<Vec<EvalCase<Setup, Expect>>>(contents) {
        return Ok(list);
    }
    // SINGLE form fallback: one `EvalCase(…)`. If this also fails, the file is malformed under both
    // shapes — surface this (single-case) parse error, named, rather than swallowing it.
    match ron::from_str::<EvalCase<Setup, Expect>>(contents) {
        Ok(case) => Ok(vec![case]),
        Err(source) => Err(EvalError::RonParse {
            path: std::path::PathBuf::from(name),
            source,
        }),
    }
}

/// Load every `*.ron` case file from `dir`, parsing each via [`parse_cases_from_str`] and flattening the
/// results — so a directory may freely mix single-case and multi-case files.
///
/// Files are read in sorted order so report ordering is deterministic across runs and machines, and a
/// multi-case file contributes its cases in authored order. A malformed `.ron` is a hard error naming
/// the file, so a typo fails the load (and any CI load test) rather than silently dropping a case.
/// Non-`.ron` entries and subdirectories are ignored.
///
/// `Setup: Default` is required because [`EvalCase::setup`] is `#[serde(default)]` (a case may omit it).
///
/// Errors as [`EvalError::Io`] (naming the directory/file that couldn't be read) or
/// [`EvalError::RonParse`] (naming the malformed file).
pub fn load_cases<Setup, Expect>(
    dir: &std::path::Path,
) -> Result<Vec<EvalCase<Setup, Expect>>, EvalError>
where
    Setup: DeserializeOwned + Default,
    Expect: DeserializeOwned,
{
    let mut paths: Vec<std::path::PathBuf> = std::fs::read_dir(dir)
        .map_err(|source| EvalError::Io {
            path: Some(dir.to_path_buf()),
            source,
        })?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("ron")))
        .collect();
    paths.sort();

    let mut cases = Vec::with_capacity(paths.len());
    for path in paths {
        let text = std::fs::read_to_string(&path).map_err(|source| EvalError::Io {
            path: Some(path.clone()),
            source,
        })?;
        // One file may hold a single case or a list; flatten either into the running list. The error
        // already names the file (the loader passes the full path as the parse `name`).
        let parsed = parse_cases_from_str::<Setup, Expect>(&path.to_string_lossy(), &text)?;
        cases.extend(parsed);
    }
    Ok(cases)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;
    use std::io::Write;

    #[derive(Debug, Default, Deserialize, PartialEq)]
    struct Setup {
        seed: u64,
    }

    #[derive(Debug, Deserialize, PartialEq)]
    struct Expect {
        contains: String,
    }

    /// `load_cases` reads every `*.ron` in sorted order, applies `Setup::default()` when `setup` is
    /// omitted, ignores non-`.ron` files, and is fail-loud on a malformed file.
    #[test]
    fn loads_sorted_with_default_setup_and_ignores_non_ron() {
        let dir = tempfile::tempdir().unwrap();

        // `b.ron` comes second in sort order but omits `setup` → it gets `Setup::default()`.
        let mut a = std::fs::File::create(dir.path().join("a.ron")).unwrap();
        write!(
            a,
            r#"(name: "alpha", instruction: "do A", setup: (seed: 5), expect: [(contains: "x")])"#
        )
        .unwrap();
        let mut b = std::fs::File::create(dir.path().join("b.ron")).unwrap();
        write!(
            b,
            r#"(name: "beta", instruction: "do B", expect: [(contains: "y")])"#
        )
        .unwrap();
        // A non-`.ron` file is ignored entirely.
        std::fs::write(dir.path().join("notes.txt"), "ignored").unwrap();

        let cases: Vec<EvalCase<Setup, Expect>> = load_cases(dir.path()).unwrap();

        assert_eq!(cases.len(), 2, "two .ron cases, the .txt ignored");
        assert_eq!(cases[0].name, "alpha");
        assert_eq!(cases[0].setup, Setup { seed: 5 });
        assert_eq!(cases[1].name, "beta");
        assert_eq!(cases[1].setup, Setup::default(), "omitted setup defaults");
    }

    /// A malformed `.ron` is a hard error naming the file (fail-loud), not a silently dropped case.
    #[test]
    fn malformed_ron_is_a_hard_error() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("bad.ron"), "this is not ron").unwrap();

        let err = load_cases::<Setup, Expect>(dir.path()).unwrap_err();
        assert!(
            err.to_string().contains("bad.ron"),
            "error names the offending file: {err}"
        );
    }

    /// `parse_cases_from_str` accepts BOTH the single-case and the list authoring shapes, and a file that
    /// is valid under neither is a hard error naming the file.
    #[test]
    fn parse_cases_handles_single_and_list_and_names_malformed_file() {
        // Single-case form (one `EvalCase(…)`), with `setup` omitted → `Setup::default()`.
        let single: Vec<EvalCase<Setup, Expect>> = parse_cases_from_str(
            "single.ron",
            r#"(name: "solo", instruction: "do it", expect: [(contains: "z")])"#,
        )
        .unwrap();
        assert_eq!(single.len(), 1);
        assert_eq!(single[0].name, "solo");
        assert_eq!(single[0].setup, Setup::default());

        // List form (`[EvalCase(…), EvalCase(…)]`) → two cases in authored order.
        let many: Vec<EvalCase<Setup, Expect>> = parse_cases_from_str(
            "many.ron",
            r#"[
                (name: "one", instruction: "first", setup: (seed: 1), expect: [(contains: "a")]),
                (name: "two", instruction: "second", expect: [(contains: "b")]),
            ]"#,
        )
        .unwrap();
        assert_eq!(many.len(), 2);
        assert_eq!(many[0].name, "one");
        assert_eq!(many[0].setup, Setup { seed: 1 });
        assert_eq!(many[1].name, "two");

        // Neither shape parses → hard error naming the file.
        let err = parse_cases_from_str::<Setup, Expect>("oops.ron", "this is not ron").unwrap_err();
        assert!(
            err.to_string().contains("oops.ron"),
            "both-parse-failed error names the file: {err}"
        );
    }

    /// A directory mixing a single-case file and a multi-case file loads the correct flattened total in
    /// sorted-by-filename order.
    #[test]
    fn load_cases_mixes_single_and_multi_case_files() {
        let dir = tempfile::tempdir().unwrap();
        // `a_single.ron` sorts first: one case.
        std::fs::write(
            dir.path().join("a_single.ron"),
            r#"(name: "solo", instruction: "alone", expect: [(contains: "x")])"#,
        )
        .unwrap();
        // `b_multi.ron` sorts second: a list of two cases.
        std::fs::write(
            dir.path().join("b_multi.ron"),
            r#"[
                (name: "m1", instruction: "first", expect: [(contains: "y")]),
                (name: "m2", instruction: "second", expect: [(contains: "z")]),
            ]"#,
        )
        .unwrap();

        let cases: Vec<EvalCase<Setup, Expect>> = load_cases(dir.path()).unwrap();
        assert_eq!(
            cases.len(),
            3,
            "1 from the single file + 2 from the multi file"
        );
        // Sorted by filename, then authored order within the multi-case file.
        let names: Vec<&str> = cases.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["solo", "m1", "m2"]);
    }
}
