//! The eval **report** types: one [`CaseOutcome`] per case and the aggregate [`EvalReport`] with a
//! readable [`Display`](std::fmt::Display) summary table.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::Duration;

/// The result of running one eval case.
///
/// `Serialize`/`Deserialize` let a host persist a run to `results/*.json` and the HTML report load it
/// back. `Duration` round-trips through serde's built-in `{secs, nanos}` form; the `predicates` tuples
/// serialize as `[label, passed]` pairs.
///
/// `#[non_exhaustive]`: likely to gain diagnostic fields. The runner builds it via [`CaseOutcome::new`]
/// (the canonical constructor) rather than a struct literal, so a new field is a one-line change there;
/// external crates that need to construct one should use [`CaseOutcome::new`] too.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct CaseOutcome {
    /// The case name (the host's case identifier).
    pub name: String,
    /// Whether EVERY expectation held (the case passed).
    pub passed: bool,
    /// Per-predicate `(label, passed)` in the case's `expect` order — for pinpointing WHICH predicate
    /// failed.
    pub predicates: Vec<(String, bool)>,
    /// Wall-clock time for the whole case run (player-context prefetch + every model turn + tool
    /// application).
    pub latency: Duration,
    /// Completion tokens summed across the case's model turns, when the backend reported `usage`;
    /// `None` when no turn carried a usage object (e.g. a `llama-server` route that omits it).
    pub tokens: Option<u32>,
    /// The tool calls the model made, as `"name(compact-args)"` strings (for debugging a failure).
    /// Excludes the harness's automatic up-front `get_player_context` prefetch.
    pub tool_calls: Vec<String>,
    /// The model's final free-text reply (the summary line), if any.
    pub final_text: Option<String>,
    /// An error from the run itself (backend failure, etc.). When `Some`, `passed` is `false` and the
    /// predicates were scored against whatever world state existed when the run aborted.
    pub error: Option<String>,
    /// The full per-case conversation transcript (every `user`/`assistant`/`tool` message), with the
    /// leading `role:"system"` message(s) STRIPPED — the system prompt is stored once at run level (see
    /// [`RunRecord::system_prompt`]). Rendered by the HTML report's per-case expander.
    ///
    /// `#[serde(default)]` is REQUIRED for backward compatibility: existing `results/*.json` were
    /// written before this field existed; without the default, the report loader
    /// ([`crate::report_html::generate_report`]) would fail to deserialize them and silently drop those
    /// runs from the report.
    #[serde(default)]
    pub transcript: Vec<Value>,
}

impl CaseOutcome {
    /// The canonical constructor, taking every field in declaration order. Used by the runner (and any
    /// external crate) so the `#[non_exhaustive]` struct can be built without a struct literal; adding a
    /// field updates this one signature.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        name: String,
        passed: bool,
        predicates: Vec<(String, bool)>,
        latency: Duration,
        tokens: Option<u32>,
        tool_calls: Vec<String>,
        final_text: Option<String>,
        error: Option<String>,
        transcript: Vec<Value>,
    ) -> Self {
        Self {
            name,
            passed,
            predicates,
            latency,
            tokens,
            tool_calls,
            final_text,
            error,
            transcript,
        }
    }
}

/// The aggregate report over a set of cases, plus latency/token statistics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalReport {
    /// One outcome per case, in run order.
    pub outcomes: Vec<CaseOutcome>,
    /// The temperature the eval ran at (noted in the summary; default 0 for determinism).
    pub temperature: f32,
    /// A short description of the backend the eval ran against (from `ChatBackend::describe`).
    pub backend: String,
    /// The exact system prompt every case in this run used (the loop's compact-vs-full selection). Stored
    /// ONCE here rather than on each [`CaseOutcome`] (it is identical across cases), and shown at the top
    /// of the run's expander in the HTML report. `#[serde(default)]` keeps old result JSON loadable.
    #[serde(default)]
    pub system_prompt: String,
}

/// One persisted eval run: an [`EvalReport`] plus the run metadata the HTML report needs to label,
/// sort, and group runs across models and over time. Serialized to
/// `results/{model}_{timestamp_file}.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunRecord {
    /// Human-friendly model label for this run (an alias like `qwen2.5-7b`, the GGUF filename for a
    /// local file, or the remote model id). Used as the comparison/grouping key.
    pub model: String,
    /// Wall-clock start of the run for display, e.g. `2026-06-18 14:03:21` (local time).
    pub timestamp_display: String,
    /// The same instant in a filesystem-safe sortable form, e.g. `20260618-140321` — used in the
    /// results filename and as a stable per-run sort key in the report.
    pub timestamp_file: String,
    /// `"local"` (a llama-server we spawned) or `"remote"` (the configured provider).
    pub backend: String,
    /// The case directory this run scored against (so a report can flag runs over different sets).
    pub cases_dir: String,
    /// The system prompt this run used, copied from [`EvalReport::system_prompt`] so the persisted
    /// record carries it at the top level too. `#[serde(default)]` keeps old result JSON loadable.
    #[serde(default)]
    pub system_prompt: String,
    /// The full eval report (per-case outcomes + aggregate stats).
    pub report: EvalReport,
}

impl EvalReport {
    /// Build a report from the per-case outcomes, recording the run temperature + backend label + the
    /// shared system prompt for the summary and the HTML report's per-run expander.
    pub fn new(
        outcomes: Vec<CaseOutcome>,
        temperature: f32,
        backend: String,
        system_prompt: String,
    ) -> Self {
        Self {
            outcomes,
            temperature,
            backend,
            system_prompt,
        }
    }

    /// Number of cases run.
    pub fn total(&self) -> usize {
        self.outcomes.len()
    }

    /// Number of cases that fully passed.
    pub fn passed(&self) -> usize {
        self.outcomes.iter().filter(|o| o.passed).count()
    }

    /// Accuracy = fraction of cases that fully passed, in `0.0..=1.0`. `0.0` for an empty report.
    pub fn accuracy(&self) -> f64 {
        if self.outcomes.is_empty() {
            0.0
        } else {
            self.passed() as f64 / self.total() as f64
        }
    }

    /// Mean per-case latency. `Duration::ZERO` for an empty report.
    pub fn mean_latency(&self) -> Duration {
        if self.outcomes.is_empty() {
            return Duration::ZERO;
        }
        let total: Duration = self.outcomes.iter().map(|o| o.latency).sum();
        total / self.outcomes.len() as u32
    }

    /// The `p`-quantile of per-case latency (`p` in `0.0..=1.0`), via the nearest-rank method on the
    /// sorted latencies. `Duration::ZERO` for an empty report.
    pub fn latency_percentile(&self, p: f64) -> Duration {
        if self.outcomes.is_empty() {
            return Duration::ZERO;
        }
        let mut lats: Vec<Duration> = self.outcomes.iter().map(|o| o.latency).collect();
        lats.sort_unstable();
        // Nearest-rank: rank = ceil(p * n), clamped to 1..=n, then 0-indexed.
        let n = lats.len();
        let rank = (p * n as f64).ceil().max(1.0) as usize;
        lats[rank.min(n) - 1]
    }

    /// Median (p50) per-case latency.
    pub fn p50_latency(&self) -> Duration {
        self.latency_percentile(0.50)
    }

    /// p95 per-case latency.
    pub fn p95_latency(&self) -> Duration {
        self.latency_percentile(0.95)
    }

    /// Total completion tokens across cases that reported a token count. `None` when NO case reported
    /// any (so the summary can say "tokens unavailable" rather than print a misleading 0).
    pub fn total_tokens(&self) -> Option<u32> {
        let reported: Vec<u32> = self.outcomes.iter().filter_map(|o| o.tokens).collect();
        if reported.is_empty() {
            None
        } else {
            Some(reported.iter().sum())
        }
    }

    /// Mean completion tokens over the cases that reported a count; `None` when none reported.
    pub fn mean_tokens(&self) -> Option<f64> {
        let reported: Vec<u32> = self.outcomes.iter().filter_map(|o| o.tokens).collect();
        if reported.is_empty() {
            None
        } else {
            Some(reported.iter().map(|&t| t as f64).sum::<f64>() / reported.len() as f64)
        }
    }
}

/// Milliseconds with one decimal, for the table (durations are short).
fn ms(d: Duration) -> String {
    format!("{:.1}ms", d.as_secs_f64() * 1000.0)
}

impl std::fmt::Display for EvalReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "AI eval report")?;
        writeln!(f, "  backend:     {}", self.backend)?;
        writeln!(f, "  temperature: {} (0 = deterministic)", self.temperature)?;
        writeln!(
            f,
            "  accuracy:    {}/{} cases passed ({:.0}%)",
            self.passed(),
            self.total(),
            self.accuracy() * 100.0
        )?;
        writeln!(
            f,
            "  latency:     mean {}  p50 {}  p95 {}",
            ms(self.mean_latency()),
            ms(self.p50_latency()),
            ms(self.p95_latency())
        )?;
        match (self.total_tokens(), self.mean_tokens()) {
            (Some(total), Some(mean)) => writeln!(
                f,
                "  tokens:      {total} completion total, {mean:.0} mean/case (cases reporting usage)"
            )?,
            _ => writeln!(
                f,
                "  tokens:      unavailable (no turn reported a `usage` object)"
            )?,
        }

        writeln!(f, "  cases:")?;
        // Column widths: name padded to the longest name for an aligned table.
        let name_w = self
            .outcomes
            .iter()
            .map(|o| o.name.len())
            .max()
            .unwrap_or(4)
            .max(4);
        for o in &self.outcomes {
            let mark = if o.passed { "PASS" } else { "FAIL" };
            let passed_preds = o.predicates.iter().filter(|(_, p)| *p).count();
            write!(
                f,
                "    [{mark}] {:<name_w$}  {}  preds {}/{}  calls {}",
                o.name,
                ms(o.latency),
                passed_preds,
                o.predicates.len(),
                o.tool_calls.len(),
            )?;
            if let Some(t) = o.tokens {
                write!(f, "  tok {t}")?;
            }
            writeln!(f)?;
            // On a failure, show what the model actually emitted, then which predicates failed +
            // any run error — so a failure is diagnosable from the text report alone (issue #1)
            // without inspecting the persisted JSON. Emitted on FAILURE only, so all-pass runs are
            // unaffected. `tool_calls` empty (model emitted nothing) simply prints no lines.
            if !o.passed {
                for call in &o.tool_calls {
                    writeln!(f, "           - emitted: {call}")?;
                }
                for (label, passed) in &o.predicates {
                    if !passed {
                        writeln!(f, "           - FAILED: {label}")?;
                    }
                }
                if let Some(err) = &o.error {
                    writeln!(f, "           - run error: {err}")?;
                }
            }
        }
        Ok(())
    }
}
