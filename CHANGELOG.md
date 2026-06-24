# Changelog

All notable changes to this project are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this
project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html) (pre-1.0: minor
versions may carry additive API and dependency changes).

## [Unreleased]

### Added
- **Opt-in panic-hook suppression** (`RunMeta::panic_suppress`). When `true`, the runner
  suppresses the global panic hook for the run loop duration and restores it on drop.
  Default `false` — enabling it during parallel evaluators is unsafe since the panic
  hook is process-global.

### Changed
- **`timestamp_file` now includes a UUID v4 suffix** to prevent same-second run
  collisions. Filenames go from `model_20260618-140321.json` to
  `model_20260618-140321-a1b2c3d4e5f64a7b8c9d0e1f2a3b4c5d.json`. The same value
  is the upload dedup key, so same-second runs now create separate server records
  instead of being deduped — this is correct behavior (they are different runs).
- **`Upload` no longer leaks the API key through `Debug`**. A custom `Debug` impl
  renders `api_key` as `"[REDACTED]"`.
- **`NoError` expectation now correctly fails when a soft run error exists**. Previously
  the runner's `artifacts.error.take()` stole the error before scoring, so `NoError`
  always saw `None` and reported passed.

### Fixed
- `RunMeta` docs: the `panic_suppress` field is no longer `#[serde(default)]` since
  `RunMeta` is not serialized.

## [0.4.0] - 2026-06-21

### Changed
- `EvalReport`'s text output now lists the tool calls the model actually emitted on a **failed**
  case — each as `- emitted: name(args)` above the failed predicates — so a failure is diagnosable
  from the text report alone, without inspecting the persisted JSON. Failure-only by design: passing
  cases are unchanged, so all-pass runs produce no extra output. (#1)

## [0.3.0] - 2026-06-21

### Added
- **Automatic upload of finished runs to EvalForge** (<https://evalforge.ai>). When a run's
  `RunMeta` carries an upload target, the assembled `RunRecord` is POSTed to
  `https://evalforge.ai/api/projects/{project_id}/runs` after the run completes, so results
  appear in the online dashboard with no manual export.
  - New `RunMeta` builder methods: `upload_to(project_id, api_key)`, `upload_from_env(project_id)`
    (reads the `EVALFORGE_API_KEY` environment variable), `upload_model(..)`, `upload_cases_dir(..)`.
  - New public `upload` module: `Upload`, `UploadResponse`, `upload_record`.
  - Configured purely at runtime — there is no cargo feature to enable. End users supply only a
    project id and an API key; the endpoint is fixed (no URL to configure).
  - Independent of `persist_to`: uploads work with or without local persistence, and when both are
    set they share one built record (one server dedup key). Re-uploads are idempotent — the server
    dedups on `project + model + timestamp`.
  - Follows the existing "warn, don't fail" posture: an upload error is logged and swallowed and
    never drops the returned `EvalReport`.
- New public persistence helpers `persist::build_record` and `persist::write_record_and_report`
  (the record-building and writing halves of `save_and_report`, now reusable by the upload path).

### Changed
- Added `ureq` (blocking HTTP, rustls TLS) as a dependency for the upload transport. It is only
  exercised at runtime when an upload target is configured.

## [0.2.0] - 2026-06-19

### Added
- Automatic run persistence: `RunMeta::persist_to(..)` writes each run as a JSON `RunRecord` and
  regenerates a self-contained `report.html` as part of the run, so hosts no longer wire that up
  themselves.

### Changed
- Rebranded the generated HTML report to EvalCore.

## [0.1.0] - 2026-06-19

### Added
- Initial release: the `Agent` / `Harness` / `Scorer` traits, the built-in `Expectation` assertion
  catalog, generic `EvalCase` / `load_cases` (single- and multi-case RON files), `run_suite` /
  `run_eval`, the `EvalReport` plus a self-contained HTML comparison report, and an 18-case
  `baseline()` capability suite.
