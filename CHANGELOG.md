# Changelog

All notable changes to this project are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this
project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html) (pre-1.0: minor
versions may carry additive API and dependency changes).

## [Unreleased]

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
