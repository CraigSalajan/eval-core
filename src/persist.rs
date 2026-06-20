//! Automatic persistence of an eval run: write it as a JSON [`RunRecord`] into a results directory
//! and (re)generate the self-contained `report.html` over every run saved there.
//!
//! This is the engine behind [`RunMeta::persist_to`](crate::RunMeta::persist_to): when a run carries a
//! [`Persist`] target, [`run_eval_with_meta`](crate::run_eval_with_meta) calls [`save_and_report`] once
//! the [`EvalReport`] is assembled, so a host gets the saved JSON + the HTML report for free — no manual
//! wiring. The filename pattern (`{slug(model)}_{timestamp_file}.json`), the pretty-printed JSON, and
//! the "warn, don't fail" posture all match the convention hosts used before this was built in, so
//! existing `results/*.json` and `report.html` stay shape-compatible.

use std::path::{Path, PathBuf};

use crate::report::{EvalReport, RunRecord};
use crate::report_html::generate_report;

/// Where and how to persist a run, attached to a [`RunMeta`](crate::RunMeta) via
/// [`RunMeta::persist_to`](crate::RunMeta::persist_to). When present on the meta passed to a run, the
/// runner writes `{slug(model)}_{timestamp_file}.json` into `results_dir` and regenerates
/// `results_dir/report.html`.
///
/// `#[non_exhaustive]`: build it through the `RunMeta` builder (`persist_to` + optional `backend_kind` /
/// `cases_dir`), never a struct literal, so new fields stay non-breaking.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Persist {
    /// Directory the per-run JSON and `report.html` are written to (created if missing).
    pub results_dir: PathBuf,
    /// The model / grouping key recorded on the run ([`RunRecord::model`](crate::report::RunRecord)),
    /// also used (slugged) in the JSON filename.
    pub model: String,
    /// The backend KIND shown in the report's Backend column
    /// ([`RunRecord::backend`](crate::report::RunRecord)), e.g. `"local"` / `"remote"`. When empty it
    /// falls back to the run's descriptive backend label ([`EvalReport::backend`]).
    pub backend: String,
    /// The case directory recorded on the run ([`RunRecord::cases_dir`](crate::report::RunRecord)),
    /// shown in the report. May be empty.
    pub cases_dir: String,
}

impl Persist {
    /// A persist target writing into `results_dir` and recording `model` as the grouping key. The
    /// backend kind and cases dir default to empty (set them via the `RunMeta` builder).
    pub(crate) fn new(results_dir: PathBuf, model: String) -> Self {
        Self {
            results_dir,
            model,
            backend: String::new(),
            cases_dir: String::new(),
        }
    }
}

/// Build a [`RunRecord`] from `report` + the [`Persist`] target, write it as
/// `{slug(model)}_{timestamp_file}.json` under `persist.results_dir`, then (re)generate `report.html`
/// over every run in that directory. Returns the written JSON path.
///
/// The record's `backend` is `persist.backend` when set, else the report's descriptive
/// [`backend`](EvalReport::backend) label; its `system_prompt` is copied from the report so a consumer
/// reading just the record sees it without descending into the nested report.
pub fn save_and_report(persist: &Persist, report: &EvalReport) -> anyhow::Result<PathBuf> {
    let (timestamp_display, timestamp_file) = timestamps();
    let backend = if persist.backend.is_empty() {
        report.backend.clone()
    } else {
        persist.backend.clone()
    };
    let record = RunRecord {
        model: persist.model.clone(),
        timestamp_display,
        timestamp_file,
        backend,
        cases_dir: persist.cases_dir.clone(),
        system_prompt: report.system_prompt.clone(),
        report: report.clone(),
    };
    let path = save_record(&persist.results_dir, &record)?;
    generate_report(&persist.results_dir)?;
    Ok(path)
}

/// Write `record` as pretty JSON to `{results_dir}/{slug(model)}_{timestamp_file}.json`, creating the
/// directory if needed, and return the written path. Public so a host can persist a hand-built
/// [`RunRecord`] without driving a full run through the runner.
pub fn save_record(results_dir: &Path, record: &RunRecord) -> anyhow::Result<PathBuf> {
    std::fs::create_dir_all(results_dir)
        .map_err(|e| anyhow::anyhow!("creating results dir {}: {e}", results_dir.display()))?;
    let file = results_dir.join(format!(
        "{}_{}.json",
        slug(&record.model),
        record.timestamp_file
    ));
    let json = serde_json::to_string_pretty(record)
        .map_err(|e| anyhow::anyhow!("serializing run record: {e}"))?;
    std::fs::write(&file, json).map_err(|e| anyhow::anyhow!("writing {}: {e}", file.display()))?;
    Ok(file)
}

/// `(display, file)` timestamps for *now* in LOCAL time, e.g.
/// `("2026-06-18 14:03:21", "20260618-140321")` — the display form labels the run in the report, the
/// file form is the filesystem-safe sortable filename/sort key.
fn timestamps() -> (String, String) {
    let now = chrono::Local::now();
    (
        now.format("%Y-%m-%d %H:%M:%S").to_string(),
        now.format("%Y%m%d-%H%M%S").to_string(),
    )
}

/// Sanitize a model label into a filesystem-safe slug for the results filename: keep `[A-Za-z0-9._-]`,
/// replace every other char with `-`, collapse runs of `-`, trim leading/trailing `-`, and fall back to
/// `"model"` if nothing is left.
pub fn slug(model: &str) -> String {
    let mut out = String::with_capacity(model.len());
    let mut last_dash = false;
    for c in model.chars() {
        if c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-' {
            out.push(c);
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    let trimmed = out.trim_matches('-').to_owned();
    if trimmed.is_empty() {
        "model".to_owned()
    } else {
        trimmed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_is_filesystem_safe_and_collapses() {
        assert_eq!(slug("Qwen/Qwen3:7b.gguf"), "Qwen-Qwen3-7b.gguf");
        assert_eq!(slug("a   b"), "a-b");
        assert_eq!(slug("***"), "model");
        assert_eq!(slug("ok_name-1.2"), "ok_name-1.2");
    }

    #[test]
    fn save_record_writes_pretty_json_with_slugged_name() {
        let dir = tempfile::tempdir().expect("tempdir");
        let report = EvalReport::new(Vec::new(), 0.0, "local: m".into(), "sys".into());
        let record = RunRecord {
            model: "my/model".into(),
            timestamp_display: "2026-06-18 14:03:21".into(),
            timestamp_file: "20260618-140321".into(),
            backend: "local".into(),
            cases_dir: "cases".into(),
            system_prompt: "sys".into(),
            report,
        };
        let path = save_record(dir.path(), &record).expect("save");
        assert_eq!(
            path.file_name().unwrap().to_string_lossy(),
            "my-model_20260618-140321.json"
        );
        let text = std::fs::read_to_string(&path).expect("read");
        assert!(
            text.contains("\n  \"model\": \"my/model\""),
            "pretty-printed"
        );
    }
}
