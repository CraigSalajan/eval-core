//! The generic benchmark **runner**: ties a [`Harness`] + [`Scorer`] over a shared `World`, runs every
//! [`EvalCase`], times each, isolates panics, and assembles an [`EvalReport`].
//!
//! This is the domain-agnostic port of AetherCore's concrete eval runner: same per-case timing, same
//! `catch_unwind` panic isolation (one bad case fails alone), same panic-hook suppression around the
//! loop, and the same stderr progress lines — but with the game/LLM specifics (which backend, which
//! world, which predicates) pushed out behind the [`Harness`]/[`Scorer`] traits.

use std::path::PathBuf;
use std::time::Instant;

use crate::case::EvalCase;
use crate::expect::Expectation;
use crate::harness::{Agent, Harness, RunArtifacts, ToolCall};
use crate::persist::{self, Persist};
use crate::report::{CaseOutcome, EvalReport};
use crate::scorer::{BuiltinScorer, Scorer};
use crate::upload::{self, Upload};

/// A stored panic hook, to be swapped back during [`PanicGuard::drop`].
type PanicHook = Box<dyn Fn(&std::panic::PanicHookInfo<'_>) + Send + Sync>;

/// RAII guard that suppresses the process-global panic hook for the duration of a run loop.
///
/// When `suppress` is `true`, `install` swaps out the hook for a no-op and returns a guard that
/// restores the previous hook on drop.  When `false` it is a no-op.
///
/// # Concurrency
///
/// The panic hook is process-global; this guard does NOT guard against concurrent use —
/// callers must not run parallel evaluators with `panic_suppress` enabled.
pub struct PanicGuard(Option<PanicHook>);

impl PanicGuard {
    /// Install a no-op panic hook when `suppress` is `true`, returning a guard that restores the
    /// previous hook on drop. A no-op (`suppress = false`) returns a `PanicGuard` that does nothing
    /// on drop.
    pub fn install(suppress: bool) -> Self {
        if suppress {
            let hook = std::panic::take_hook();
            std::panic::set_hook(Box::new(|_| {}));
            PanicGuard(Some(hook))
        } else {
            PanicGuard(None)
        }
    }
}

impl std::fmt::Debug for PanicGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PanicGuard").finish_non_exhaustive()
    }
}

impl Drop for PanicGuard {
    fn drop(&mut self) {
        if let Some(hook) = self.0.take() {
            std::panic::set_hook(hook);
        }
    }
}

/// Run-level metadata recorded on the [`EvalReport`] but NOT intrinsic to the generic runner.
///
/// [`EvalReport`] carries a few fields that are meaningful for LLM/agent runs (the sampling
/// `temperature`, a `backend` label, the shared `system_prompt`) but have no generic meaning here. Rather
/// than hardcode LLM assumptions into [`run_eval`], the host supplies them via this struct; the report's
/// serialized shape (and therefore the HTML report + saved JSON) is unchanged. A host with no notion of
/// these can use [`RunMeta::default`] (temperature `0.0`, empty `backend`/`system_prompt`).
///
/// `#[non_exhaustive]`: new run-level metadata can be added without a breaking change. Build it with
/// `RunMeta::new(temperature, backend, system_prompt)`.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct RunMeta {
    /// Sampling temperature the run used, recorded in the report summary. Neutral default `0.0`.
    pub temperature: f32,
    /// A short description of what was benchmarked (e.g. the backend/model label). Neutral default `""`.
    pub backend: String,
    /// A run-level prompt/preamble shared across all cases, stored once on the report (shown at the top
    /// of the HTML report's per-run expander). Neutral default `""`.
    pub system_prompt: String,
    /// When set, the run is auto-persisted: after the cases finish, the [`EvalReport`] is written as a
    /// JSON [`RunRecord`](crate::report::RunRecord) into the [`Persist::results_dir`] and
    /// `report.html` is regenerated over every run there. `None` (the default) means compute-only — no
    /// disk I/O. Set it with [`RunMeta::persist_to`] (+ optional [`backend_kind`](RunMeta::backend_kind)
    /// / [`cases_dir`](RunMeta::cases_dir)).
    pub persist: Option<Persist>,
    /// When set, the run is auto-uploaded: after the cases finish, the assembled
    /// [`RunRecord`](crate::report::RunRecord) is POSTed to the EvalForge API (evalforge.ai) so it
    /// shows up in the online dashboard. `None` (the default) means no upload. Set it with
    /// [`upload_to`](RunMeta::upload_to) / [`upload_from_env`](RunMeta::upload_from_env) (+ optional
    /// [`upload_model`](RunMeta::upload_model) / [`upload_cases_dir`](RunMeta::upload_cases_dir) for the
    /// upload-only, no-`persist_to` case).
    pub upload: Option<Upload>,
    /// When `true`, the runner suppresses the global panic hook for the duration of the entire
    /// run loop so that raw panic messages aren't double-printed — the per-case AFTER line and
    /// the recorded `CaseOutcome.error` are the sole panic source. This requires a process-global
    /// hook swap, which means it MUST NOT be enabled concurrently (e.g. by parallel runners).
    /// Default `false`.
    pub panic_suppress: bool,
}

impl RunMeta {
    /// Create a new `RunMeta` with the given temperature, backend label, and system prompt.
    pub fn new(
        temperature: f32,
        backend: impl Into<String>,
        system_prompt: impl Into<String>,
    ) -> Self {
        Self {
            temperature,
            backend: backend.into(),
            system_prompt: system_prompt.into(),
            persist: None,
            upload: None,
            panic_suppress: false,
        }
    }

    /// Enable automatic persistence for this run: after the cases finish, write the run as
    /// `{model}_{timestamp}.json` into `results_dir` and (re)generate `results_dir/report.html` over
    /// every run saved there. This is what turns a compute-only run into one that saves its JSON + the
    /// HTML report as part of the call — the host no longer wires that up itself.
    pub fn persist_to(mut self, results_dir: impl Into<PathBuf>, model: impl Into<String>) -> Self {
        self.persist = Some(Persist::new(results_dir.into(), model.into()));
        self
    }

    /// Record the backend KIND on the persisted run — the report's Backend column, e.g. `"local"` /
    /// `"remote"` — distinct from the descriptive `backend` label carried on the report. No-op unless
    /// [`persist_to`](Self::persist_to) enabled persistence first.
    pub fn backend_kind(mut self, kind: impl Into<String>) -> Self {
        if let Some(p) = self.persist.as_mut() {
            p.backend = kind.into();
        }
        self
    }

    /// Record the case directory on the persisted run (shown in the report). No-op unless
    /// [`persist_to`](Self::persist_to) enabled persistence first.
    pub fn cases_dir(mut self, dir: impl Into<String>) -> Self {
        if let Some(p) = self.persist.as_mut() {
            p.cases_dir = dir.into();
        }
        self
    }

    /// Enable automatic upload for this run: after the cases finish, POST the assembled
    /// [`RunRecord`](crate::report::RunRecord) to the EvalForge API under `project_id`, authenticating
    /// with `api_key`. Independent of [`persist_to`](Self::persist_to) — works with or without it; when
    /// both are set, the saved file and the uploaded record share one record (one timestamp / dedup
    /// key). An upload failure is warned, never fatal, so it can't drop the eval signal. The endpoint is
    /// fixed to evalforge.ai (no URL to configure).
    ///
    /// ```no_run
    /// use eval_core::{run_suite_with_meta, RunMeta};
    /// # fn demo(agent: &impl eval_core::Agent, cases: &[eval_core::EvalCase<(), eval_core::Expectation>]) {
    /// // Persist locally AND upload to EvalForge — persist's identity is reused for the upload.
    /// let meta = RunMeta::new(0.0, "local: my-model", "sys")
    ///     .persist_to("eval/results", "my-model")
    ///     .backend_kind("local")
    ///     .upload_to("00000000-0000-0000-0000-000000000000", "sk-eval-...");
    /// let _ = run_suite_with_meta(agent, cases, meta);
    /// # }
    /// ```
    pub fn upload_to(mut self, project_id: impl Into<String>, api_key: impl Into<String>) -> Self {
        self.upload = Some(Upload::new(project_id.into(), api_key.into()));
        self
    }

    /// As [`upload_to`](Self::upload_to), but reads the API key from the `EVALFORGE_API_KEY` environment
    /// variable instead of taking it inline. If the variable is unset or empty, upload is left disabled
    /// (`upload = None`) with a warning rather than failing — the builder stays infallible.
    ///
    /// ```no_run
    /// use eval_core::{run_suite_with_meta, RunMeta};
    /// # fn demo(agent: &impl eval_core::Agent, cases: &[eval_core::EvalCase<(), eval_core::Expectation>]) {
    /// // Upload-only (no local persist), key from EVALFORGE_API_KEY.
    /// let meta = RunMeta::new(0.0, "remote: my-model", "sys")
    ///     .upload_from_env("00000000-0000-0000-0000-000000000000")
    ///     .upload_model("my-model"); // record identity, since there is no persist target
    /// let _ = run_suite_with_meta(agent, cases, meta);
    /// # }
    /// ```
    pub fn upload_from_env(mut self, project_id: impl Into<String>) -> Self {
        match std::env::var("EVALFORGE_API_KEY") {
            Ok(key) if !key.trim().is_empty() => {
                self.upload = Some(Upload::new(project_id.into(), key));
            }
            _ => {
                tracing::warn!("EVALFORGE_API_KEY not set (or empty); upload disabled");
                eprintln!("warning: EVALFORGE_API_KEY not set (or empty); upload disabled");
            }
        }
        self
    }

    /// Record the model / grouping key on the uploaded run. Only used when there is NO
    /// [`persist_to`](Self::persist_to) target (otherwise persist's model is reused). No-op unless
    /// [`upload_to`](Self::upload_to) / [`upload_from_env`](Self::upload_from_env) enabled upload first.
    pub fn upload_model(mut self, model: impl Into<String>) -> Self {
        if let Some(u) = self.upload.as_mut() {
            u.model = model.into();
        }
        self
    }

    /// Record the case directory on the uploaded run. Only used when there is NO
    /// [`persist_to`](Self::persist_to) target (otherwise persist's cases dir is reused). No-op unless
    /// [`upload_to`](Self::upload_to) / [`upload_from_env`](Self::upload_from_env) enabled upload first.
    pub fn upload_cases_dir(mut self, dir: impl Into<String>) -> Self {
        if let Some(u) = self.upload.as_mut() {
            u.cases_dir = dir.into();
        }
        self
    }
}

/// Run every `case` through `harness` + `scorer` and aggregate into an [`EvalReport`], using default
/// [`RunMeta`] (temperature `0`, empty backend/system-prompt labels).
///
/// This is the simple entry point for hosts that don't track LLM run metadata. For LLM/agent runs that
/// want the report to record a backend label, temperature, or shared system prompt, use
/// [`run_eval_with_meta`].
///
/// Semantics per case: build a fresh world via [`Harness::setup`], time [`Harness::run`] (wall-clock for
/// the whole run), then score every predicate via [`Scorer::score`]. A case PASSES iff `run` returned
/// `Ok` AND every predicate passed. The whole build+run+score is isolated behind `catch_unwind`, so a
/// panicking case fails only itself (with the panic message recorded in
/// [`CaseOutcome::error`]). Progress is emitted to stderr.
pub fn run_eval<H, S>(
    harness: &H,
    scorer: &S,
    cases: &[EvalCase<H::Setup, S::Expect>],
) -> EvalReport
where
    H: Harness,
    S: Scorer<World = H::World>,
{
    run_eval_with_meta(harness, scorer, cases, RunMeta::default())
}

/// As [`run_eval`], but with explicit run [`RunMeta`] (backend label, temperature, shared system
/// prompt) recorded on the resulting [`EvalReport`].
///
/// This is the single convergence point that owns the progress logging: a one-time startup banner, then
/// a `[i/total]` line BEFORE and AFTER each case (the BEFORE line — the anti-hang signal — prints the
/// instant the case starts). ALL progress goes to stderr; stdout is left clean for any report payload a
/// host wants to print.
pub fn run_eval_with_meta<H, S>(
    harness: &H,
    scorer: &S,
    cases: &[EvalCase<H::Setup, S::Expect>],
    meta: RunMeta,
) -> EvalReport
where
    H: Harness,
    S: Scorer<World = H::World>,
{
    let total = cases.len();
    eprintln!("── eval run ──────────────────────────────────────────");
    if !meta.backend.is_empty() {
        eprintln!("backend:     {}", meta.backend);
    }
    eprintln!("cases:       {total}");
    eprintln!("temperature: {}", meta.temperature);
    eprintln!("──────────────────────────────────────────────────────");

    // Suppress the process-global panic hook only when opt-in, so the restore is safe even
    // for parallel / concurrent eval runs (the global hook swap is guarded by a Mutex).
    let _panic_guard = PanicGuard::install(meta.panic_suppress);

    let outcomes: Vec<CaseOutcome> = cases
        .iter()
        .enumerate()
        .map(|(idx, case)| {
            let i = idx + 1;
            // BEFORE: which case is in flight, printed the instant the run starts (anti-hang signal).
            eprintln!("[{i}/{total}] {} …", case.name);
            let outcome = run_case(harness, scorer, case);
            // AFTER: result + timing, so a slow case is visibly distinguishable from a hung one.
            let status = if outcome.passed { "PASS" } else { "FAIL" };
            let tokens = outcome
                .tokens
                .map_or_else(|| "?".to_owned(), |t| t.to_string());
            let detail = match &outcome.error {
                Some(err) => format!(", {}", truncate_one_line(err)),
                None => String::new(),
            };
            eprintln!(
                "[{i}/{total}] {} … {status} ({:.1}s, {tokens} tok{detail})",
                case.name,
                outcome.latency.as_secs_f64()
            );
            outcome
        })
        .collect();

    let report = EvalReport::new(outcomes, meta.temperature, meta.backend, meta.system_prompt);

    // Auto-persist (write JSON + regenerate the HTML report) and/or auto-upload to EvalForge when the
    // run carries a target. The `RunRecord` is built ONCE and fanned out to both sinks so the saved file
    // and the uploaded record share one timestamp (one dedup key). Each sink "warns, doesn't fail": a
    // persistence OR an upload failure must NOT lose the eval signal, so it is warned and swallowed (the
    // report is still returned). This is what makes saving / reporting / uploading automatic for any
    // host that set `persist_to` / `upload_to`.
    let do_persist = meta.persist.is_some();
    let do_upload = meta.upload.is_some();
    if do_persist || do_upload {
        // Resolve the record identity: prefer persist's values, else the upload-only values, else fall
        // back to the report's own (an empty `backend_kind` makes `build_record` use the report label).
        let model;
        let backend_kind;
        let cases_dir;
        if let Some(p) = &meta.persist {
            model = p.model.clone();
            backend_kind = p.backend.clone();
            cases_dir = p.cases_dir.clone();
        } else {
            let u = meta
                .upload
                .as_ref()
                .expect("do_persist || do_upload with no persist implies upload is Some");
            model = u.model.clone();
            backend_kind = u.backend.clone();
            cases_dir = u.cases_dir.clone();
        }

        let record = persist::build_record(model, backend_kind, cases_dir, &report);

        if let Some(persist) = &meta.persist {
            match persist::write_record_and_report(&persist.results_dir, &record) {
                Ok(path) => eprintln!("saved run + report: {}", path.display()),
                Err(e) => {
                    tracing::warn!("auto-persist failed: {e}");
                    eprintln!("warning: failed to persist run / generate report: {e}");
                }
            }
        }

        if let Some(upload) = &meta.upload {
            match upload::upload_record(upload, &record) {
                Ok(r) => eprintln!(
                    "uploaded run to evalforge: id={} deduped={}",
                    r.run_id, r.deduped
                ),
                Err(e) => {
                    tracing::warn!("upload failed: {e}");
                    eprintln!("warning: failed to upload run to evalforge: {e}");
                }
            }
        }
    }
    report
}

/// Build the world, run one case against it, and score it into a [`CaseOutcome`].
///
/// The whole build+run+score is wrapped in `catch_unwind`: the world is freshly built per case, so
/// discarding a half-built world on a panic is safe, and `&H`/`&S` are shared references the closure only
/// reads — hence the `AssertUnwindSafe` boundary. On a panic, the case is marked failed, every predicate
/// is marked failed (with its scorer-derived label where we can still produce one — but a panic may have
/// happened mid-scoring, so we fall back to a positional label), and the panic message is recorded.
fn run_case<H, S>(harness: &H, scorer: &S, case: &EvalCase<H::Setup, S::Expect>) -> CaseOutcome
where
    H: Harness,
    S: Scorer<World = H::World>,
{
    let started = Instant::now();

    let scored = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // Build a fresh world for this case, run the instruction (timing the whole run), then score.
        let mut world = harness.setup(&case.setup);
        let run = harness.run(&case.instruction, &mut world);
        let latency = started.elapsed();

        // The harness can report a run-level error two ways: an `Err` return (hard failure) or
        // `RunArtifacts.error` (a soft, captured error). Prefer the `Err`; either fails the case.
        // Take the soft error for the run-level signal, but restore it on `artifacts` so the
        // scoring step (e.g. `Expectation::NoError`) can still see that a soft error occurred.
        let (artifacts, run_error) = match run {
            Ok(mut artifacts) => {
                let had_error = artifacts.error.take();
                artifacts.error = had_error.clone();
                (artifacts, had_error)
            }
            Err(e) => (RunArtifacts::default(), Some(e.to_string())),
        };

        // Score every predicate against the resulting world AND the run's artifacts, keeping each
        // `(label, passed)`.
        let predicates: Vec<(String, bool)> = case
            .expect
            .iter()
            .map(|exp| scorer.score(exp, &artifacts, &world))
            .collect();

        (artifacts, run_error, latency, predicates)
    }));

    match scored {
        Ok((artifacts, run_error, latency, predicates)) => {
            // A case passes iff the run didn't error AND every predicate held. (A scored-but-error'd run
            // can't pass — partial state is unreliable — but we still record which predicates held.)
            let passed = run_error.is_none() && predicates.iter().all(|(_, p)| *p);
            // The report keeps tool calls as DISPLAY strings (shape-compatible with the saved JSON /
            // `--json` / HTML report). Derive them here from the structured `ToolCall`s the artifacts
            // carry, so a host never formats them itself.
            let tool_calls: Vec<String> =
                artifacts.tool_calls.iter().map(ToolCall::display).collect();
            CaseOutcome::new(
                case.name.clone(),
                passed,
                predicates,
                latency,
                artifacts.tokens,
                tool_calls,
                artifacts.final_text,
                run_error,
                artifacts.transcript,
            )
        }
        Err(payload) => {
            // Recover a message from the panic payload (most panics carry a `&str` or `String`).
            let msg = payload
                .downcast_ref::<&str>()
                .map(|s| (*s).to_owned())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "unknown panic".to_owned());
            // A panic may have struck anywhere in build/run/score, so we can't trust any partial
            // per-predicate labels. Record one failed predicate per `expect` with a positional label.
            let predicates = (0..case.expect.len())
                .map(|i| (format!("predicate #{i}"), false))
                .collect();
            CaseOutcome::new(
                case.name.clone(),
                false,
                predicates,
                started.elapsed(),
                None,
                Vec::new(),
                None,
                Some(format!("panic: {msg}")),
                Vec::new(),
            )
        }
    }
}

/// An internal adapter turning an [`Agent`] into a `Harness<World = (), Setup = ()>` so the easy
/// [`run_suite`] path reuses the exact same panic-isolated runner as the full [`Harness`] path.
///
/// A deliberate explicit struct rather than a blanket `impl<T: Agent> Harness for T`: a blanket impl
/// would conflict (coherence) with a host's own `Harness` impl (e.g. AetherCore's `AetherHarness`), so
/// the adapter keeps the two trait families independent. The world is `()` (built once per case, inert)
/// and the agent's run failure is mapped onto [`Harness::run`]'s `anyhow::Result`.
#[derive(Debug)]
pub struct AgentHarness<'a, A: Agent> {
    agent: &'a A,
}

impl<'a, A: Agent> AgentHarness<'a, A> {
    /// Wrap an agent reference as a `Harness`.
    pub fn new(agent: &'a A) -> Self {
        Self { agent }
    }
}

impl<A: Agent> Harness for AgentHarness<'_, A> {
    type World = ();
    type Setup = ();

    fn setup(&self, _setup: &()) {}

    fn run(&self, instruction: &str, _world: &mut ()) -> anyhow::Result<RunArtifacts> {
        // Map the public `EvalError` to the runner's internal `anyhow::Result` (an `Err` fails the
        // case and records the message, just like a `Harness` returning `Err`).
        self.agent
            .run(instruction)
            .map_err(|e| anyhow::anyhow!(e.to_string()))
    }
}

/// The easy path: run a suite of [`Expectation`]-based cases against an [`Agent`], scoring with the
/// built-in [`BuiltinScorer`] — no `World`, no `Setup`, no host `Scorer` impl.
///
/// Implement [`Agent::run`] for your harness, author `EvalCase<(), Expectation>` cases (in RON or
/// inline), and call this; you get back the same [`EvalReport`] as the full path. Uses default
/// [`RunMeta`]; for a backend/temperature label use [`run_suite_with_meta`].
pub fn run_suite(agent: &impl Agent, cases: &[EvalCase<(), Expectation>]) -> EvalReport {
    run_suite_with_meta(agent, cases, RunMeta::default())
}

/// As [`run_suite`], but records explicit [`RunMeta`] (backend label, temperature, shared system prompt)
/// on the report.
pub fn run_suite_with_meta(
    agent: &impl Agent,
    cases: &[EvalCase<(), Expectation>],
    meta: RunMeta,
) -> EvalReport {
    let harness = AgentHarness::new(agent);
    run_eval_with_meta(&harness, &BuiltinScorer, cases, meta)
}

/// Collapse a (possibly multi-line) error/panic message to a single, length-bounded line for the live
/// per-case AFTER line: take only up to the first newline, then truncate to `MAX` chars (on a char
/// boundary, since panic messages can contain non-ASCII) with an ellipsis. The full message is still
/// preserved verbatim in [`CaseOutcome::error`].
fn truncate_one_line(msg: &str) -> String {
    const MAX: usize = 120;
    let first_line = msg.lines().next().unwrap_or("");
    if first_line.chars().count() <= MAX {
        first_line.to_owned()
    } else {
        let truncated: String = first_line.chars().take(MAX).collect();
        format!("{truncated}…")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A world that is just a flag set by the harness, scored by a closure-free predicate.
    #[derive(Default)]
    struct W {
        ok: bool,
    }

    /// `Setup` placeholder. The per-case behavior is keyed off the instruction string, so the setup
    /// itself is inert; it only needs to satisfy [`Harness::Setup`].
    #[derive(Clone, Copy, Default)]
    struct How;

    struct H;

    impl Harness for H {
        type World = W;
        type Setup = How;

        fn setup(&self, _setup: &How) -> W {
            W::default()
        }

        fn run(&self, instruction: &str, world: &mut W) -> anyhow::Result<RunArtifacts> {
            // The behavior is keyed off the instruction string the test sets per case.
            match instruction {
                "pass" => {
                    world.ok = true;
                    Ok(RunArtifacts {
                        tokens: Some(7),
                        ..RunArtifacts::default()
                    })
                }
                "soft" => {
                    world.ok = true; // predicate would pass, but the soft error still fails the case.
                    Ok(RunArtifacts {
                        error: Some("soft boom".to_owned()),
                        ..RunArtifacts::default()
                    })
                }
                "hard" => anyhow::bail!("hard boom"),
                "panic" => panic!("kaboom"),
                other => panic!("unexpected instruction {other}"),
            }
        }
    }

    struct Sc;

    impl Scorer for Sc {
        type World = W;
        type Expect = ();

        fn score(&self, _expect: &(), _artifacts: &RunArtifacts, world: &W) -> (String, bool) {
            ("world.ok".to_owned(), world.ok)
        }
    }

    fn case(name: &str, instruction: &str) -> EvalCase<How, ()> {
        EvalCase {
            name: name.to_owned(),
            instruction: instruction.to_owned(),
            setup: How,
            expect: vec![()],
        }
    }

    #[test]
    fn pass_soft_hard_and_panic_are_isolated() {
        let cases = vec![
            case("pass", "pass"),
            case("soft", "soft"),
            case("hard", "hard"),
            case("panic", "panic"),
        ];

        let report = run_eval(&H, &Sc, &cases);

        assert_eq!(report.total(), 4);
        assert_eq!(report.passed(), 1, "only the clean run passes");

        // Pass: ok, no error, token count forwarded, predicate held.
        let pass = &report.outcomes[0];
        assert!(pass.passed);
        assert_eq!(pass.tokens, Some(7));
        assert!(pass.error.is_none());
        assert_eq!(pass.predicates, vec![("world.ok".to_owned(), true)]);

        // Soft error: predicate held but the captured error still fails the case.
        let soft = &report.outcomes[1];
        assert!(!soft.passed);
        assert_eq!(soft.error.as_deref(), Some("soft boom"));
        assert!(soft.predicates[0].1, "predicate itself held");

        // Hard error (`Err` return): failed, error recorded, world scored as built (not ok).
        let hard = &report.outcomes[2];
        assert!(!hard.passed);
        assert_eq!(hard.error.as_deref(), Some("hard boom"));
        assert!(!hard.predicates[0].1);

        // Panic: isolated to this case, recorded as a `panic:`-prefixed error, predicate forced failed.
        let panicked = &report.outcomes[3];
        assert!(!panicked.passed);
        assert!(
            panicked
                .error
                .as_deref()
                .is_some_and(|e| e.contains("kaboom")),
            "panic message captured: {:?}",
            panicked.error
        );
        assert_eq!(panicked.predicates.len(), 1);
        assert!(!panicked.predicates[0].1);
    }

    /// NoError expectation must fail when a soft error occurred — the soft error is taken from
    /// artifacts before scoring, but restored so that `Expectation::NoError` can still see it.
    #[test]
    fn no_error_fails_on_soft_error() {
        use crate::expect::Expectation;

        struct SH;

        impl Harness for SH {
            type World = ();
            type Setup = ();

            fn setup(&self, _setup: &()) {}

            fn run(&self, _instruction: &str, _world: &mut ()) -> anyhow::Result<RunArtifacts> {
                Ok(RunArtifacts {
                    error: Some("soft boom".to_owned()),
                    ..RunArtifacts::default()
                })
            }
        }

        struct SSc;

        impl Scorer for SSc {
            type World = ();
            type Expect = Expectation;

            fn score(
                &self,
                expect: &Expectation,
                artifacts: &RunArtifacts,
                _world: &(),
            ) -> (String, bool) {
                expect
                    .evaluate(artifacts)
                    .unwrap_or((expect.label().to_owned(), false))
            }
        }

        let cases = vec![EvalCase {
            name: "soft-error-no-error".to_owned(),
            instruction: "".to_owned(),
            setup: (),
            expect: vec![Expectation::NoError],
        }];

        let report = run_eval(&SH, &SSc, &cases);
        assert_eq!(report.total(), 1);
        assert!(
            !report.outcomes[0].passed,
            "NoError must fail when a soft error exists"
        );
        assert!(
            !report.outcomes[0].predicates[0].1,
            "NoError predicate must be false when a soft error occurred"
        );
    }

    /// A run carrying a `persist_to` target writes its `{slug(model)}_{timestamp}.json` AND regenerates
    /// `report.html` in the target dir as part of the run — no separate host call.
    #[test]
    fn persist_to_writes_run_json_and_report_html() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cases = vec![case("pass", "pass")];
        let meta = RunMeta::new(0.0, "local: m", "sys")
            .persist_to(dir.path(), "my-model")
            .backend_kind("local")
            .cases_dir("eval/cases");
        let _ = run_eval_with_meta(&H, &Sc, &cases, meta);

        let names: Vec<String> = std::fs::read_dir(dir.path())
            .expect("read results dir")
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert!(
            names
                .iter()
                .any(|n| n.starts_with("my-model_") && n.ends_with(".json")),
            "per-run JSON written with slugged model name; got {names:?}"
        );
        assert!(
            names.iter().any(|n| n == "report.html"),
            "report.html regenerated; got {names:?}"
        );
    }

    /// The easy `Agent` + `run_suite` path: one `Agent::run`, built-in `Expectation`s, no `World`,
    /// `Setup`, or `Scorer`. Exercises tool-call + final-text assertions and the structured-→display
    /// derivation in the runner.
    #[test]
    fn run_suite_scores_builtin_expectations_over_agent_artifacts() {
        use crate::expect::Expectation;
        use crate::harness::ToolCall;
        use serde_json::json;

        // A fake agent: if asked to "add", it emits a calculator call and ends with the sum; otherwise
        // it just echoes (no tool call), and "boom" reports a run failure.
        struct FakeAgent;
        impl Agent for FakeAgent {
            fn run(&self, instruction: &str) -> Result<RunArtifacts, crate::error::EvalError> {
                if instruction == "boom" {
                    return Err(crate::error::EvalError::agent("backend down"));
                }
                if instruction.contains("add") {
                    Ok(RunArtifacts {
                        tool_calls: vec![ToolCall::new(
                            "calculator",
                            json!({"op": "add", "a": 2, "b": 2}),
                        )],
                        final_text: Some("The answer is 4".to_owned()),
                        ..RunArtifacts::default()
                    })
                } else {
                    Ok(RunArtifacts {
                        final_text: Some(instruction.to_owned()),
                        ..RunArtifacts::default()
                    })
                }
            }
        }

        fn case(
            name: &str,
            instruction: &str,
            expect: Vec<Expectation>,
        ) -> EvalCase<(), Expectation> {
            EvalCase {
                name: name.to_owned(),
                instruction: instruction.to_owned(),
                setup: (),
                expect,
            }
        }

        let cases = vec![
            case(
                "adds",
                "please add 2 and 2",
                vec![
                    Expectation::CalledToolWith {
                        tool: "calculator".into(),
                        args: json!({"op": "add"}),
                    },
                    Expectation::FinalNumberEquals {
                        value: 4.0,
                        tolerance: 0.0,
                    },
                ],
            ),
            case(
                "no-tools",
                "hello there",
                vec![
                    Expectation::NoToolCalls,
                    Expectation::FinalTextContains {
                        text: "hello".into(),
                        case_insensitive: false,
                    },
                ],
            ),
            case("fails-run", "boom", vec![Expectation::NoError]),
        ];

        let report = run_suite(&FakeAgent, &cases);
        assert_eq!(report.total(), 3);
        assert_eq!(
            report.passed(),
            2,
            "the two well-behaved cases pass; the run-error case fails"
        );

        // The structured calls were rendered to the report's display strings.
        let adds = &report.outcomes[0];
        assert!(adds.passed);
        assert_eq!(adds.tool_calls.len(), 1);
        assert!(
            adds.tool_calls[0].starts_with("calculator("),
            "display string derived from the structured ToolCall: {:?}",
            adds.tool_calls
        );

        let boom = &report.outcomes[2];
        assert!(!boom.passed);
        assert!(
            boom.error
                .as_deref()
                .is_some_and(|e| e.contains("backend down"))
        );
    }
}
