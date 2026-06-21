//! Automatic upload of a finished eval run to the EvalForge API (evalforge.ai) so results show up
//! in the online dashboard with no manual export/import.
//!
//! This is the engine behind [`RunMeta::upload_to`](crate::RunMeta::upload_to): when a run carries an
//! [`Upload`] target, [`run_eval_with_meta`](crate::run_eval_with_meta) POSTs the assembled
//! [`RunRecord`] to `https://evalforge.ai/api/projects/{project_id}/runs` once the run finishes. The
//! record's serde shape IS the API's ingest DTO, so the body is reused as-is — no separate wire type.
//! The module is always compiled in; uploading happens only at runtime when a host configures an
//! [`Upload`] target on the run metadata (nothing is sent otherwise).
//!
//! The base URL is a hardcoded crate constant (`EVALFORGE_BASE_URL`); end users only ever configure a
//! project id + API key. (A `#[cfg(test)]`-only override exists so unit tests can point at a local mock;
//! it is NOT part of the public API.)

use std::time::Duration;

use serde::Deserialize;

use crate::report::RunRecord;

/// The single hardcoded EvalForge host. End users never supply a URL; uploads always go here.
const EVALFORGE_BASE_URL: &str = "https://evalforge.ai";

/// Where and how to upload a run, attached to a [`RunMeta`](crate::RunMeta) via
/// [`RunMeta::upload_to`](crate::RunMeta::upload_to). When present on the meta passed to a run, the
/// runner POSTs the assembled [`RunRecord`] to the EvalForge API after the cases finish.
///
/// `#[non_exhaustive]`: build it through the `RunMeta` builder (`upload_to` / `upload_from_env` +
/// optional `upload_model` / `upload_cases_dir`), never a struct literal, so new fields stay
/// non-breaking.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Upload {
    /// The EvalForge Project UUID the run is uploaded under (the URL path param). User-supplied.
    pub project_id: String,
    /// The account-level API key, sent as `Authorization: Bearer {api_key}`. User-supplied.
    pub api_key: String,
    /// The model / grouping key recorded on the uploaded [`RunRecord`]. Used ONLY when the run has no
    /// [`Persist`](crate::Persist) target; when persist is set, persist's identity is reused so the
    /// saved file and the uploaded record share one dedup key. Set via
    /// [`upload_model`](crate::RunMeta::upload_model).
    pub model: String,
    /// The backend KIND recorded on the uploaded record (the report's Backend column). Used ONLY when
    /// persist is absent; when empty the record falls back to the report's descriptive backend label.
    pub backend: String,
    /// The case directory recorded on the uploaded record. Used ONLY when persist is absent. Set via
    /// [`upload_cases_dir`](crate::RunMeta::upload_cases_dir).
    pub cases_dir: String,
    /// The API base URL. Always `EVALFORGE_BASE_URL` in production — crate-private with NO public
    /// setter; only a `#[cfg(test)]` helper can override it (so unit tests can point at a local mock).
    base_url: String,
}

impl Upload {
    /// An upload target POSTing to EvalForge under `project_id`, authenticating with `api_key`. The
    /// record identity (model / backend / cases dir) defaults to empty (set them via the `RunMeta`
    /// builder for the upload-only case); the base URL is fixed to `EVALFORGE_BASE_URL`.
    pub(crate) fn new(project_id: String, api_key: String) -> Self {
        Self {
            project_id,
            api_key,
            model: String::new(),
            backend: String::new(),
            cases_dir: String::new(),
            base_url: EVALFORGE_BASE_URL.to_owned(),
        }
    }

    /// Test-only override of the otherwise-fixed base URL, so a unit test can point an upload at a local
    /// mock server. This is the ONLY way to change `base_url` and it is not part of the public API.
    #[cfg(test)]
    pub(crate) fn with_base_url(mut self, base_url: String) -> Self {
        self.base_url = base_url;
        self
    }
}

/// The EvalForge ingest URL for `project_id`: `{base_url}/api/projects/{project_id}/runs`. A free fn so
/// it is unit-testable; a trailing slash on `base_url` is trimmed defensively to avoid a double `//`.
fn endpoint_url(base_url: &str, project_id: &str) -> String {
    let base = base_url.trim_end_matches('/');
    format!("{base}/api/projects/{project_id}/runs")
}

/// The EvalForge ingest success body: `{ "runId": string, "deduped": bool }`. `deduped` is `true` when
/// the server replaced a prior run with the same `(projectId, model, timestamp_file)` (re-uploads are
/// safe / idempotent).
#[derive(Debug, Clone, Deserialize)]
pub struct UploadResponse {
    /// The server-assigned id of the (created or replaced) run.
    #[serde(rename = "runId")]
    pub run_id: String,
    /// `true` if this upload replaced an existing run with the same dedup key, `false` if newly created.
    #[serde(default)]
    pub deduped: bool,
}

/// POST `record` to the EvalForge ingest endpoint for `upload`, returning the parsed [`UploadResponse`].
///
/// Uses a blocking `ureq` agent with a ~30s timeout so a hung network can't block the run return. The
/// body is the JSON-serialized [`RunRecord`] (its serde shape IS the API DTO); auth is
/// `Authorization: Bearer {api_key}`. A non-2xx status, a transport error, or an unparseable body all
/// map to a descriptive [`anyhow::Error`] — the caller "warns, doesn't fail", so an upload error never
/// drops the eval signal.
//
// NOTE (v1): no retries. The server dedups on `(projectId, model, timestamp_file)`, so a retry/backoff
// on 5xx / transport errors would be safe to add as a follow-up — kept minimal here.
pub fn upload_record(upload: &Upload, record: &RunRecord) -> anyhow::Result<UploadResponse> {
    let url = endpoint_url(&upload.base_url, &upload.project_id);
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(30))
        .build();
    let response = agent
        .post(&url)
        .set("Authorization", &format!("Bearer {}", upload.api_key))
        .set("Content-Type", "application/json")
        .send_json(record);
    match response {
        Ok(response) => response
            .into_json::<UploadResponse>()
            .map_err(|e| anyhow::anyhow!("parsing upload response: {e}")),
        Err(ureq::Error::Status(code, response)) => {
            let body = response.into_string().unwrap_or_default();
            Err(anyhow::anyhow!(
                "evalforge upload failed: HTTP {code}: {body}"
            ))
        }
        Err(ureq::Error::Transport(t)) => {
            Err(anyhow::anyhow!("evalforge upload transport error: {t}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    use super::*;
    use crate::report::{EvalReport, RunRecord};

    #[test]
    fn endpoint_url_is_evalforge_projects_runs() {
        assert_eq!(
            endpoint_url("https://evalforge.ai", "abc"),
            "https://evalforge.ai/api/projects/abc/runs"
        );
    }

    #[test]
    fn endpoint_url_trims_trailing_slash() {
        assert_eq!(
            endpoint_url("https://evalforge.ai/", "abc"),
            "https://evalforge.ai/api/projects/abc/runs"
        );
    }

    /// Full HTTP round-trip against a single-shot, `std`-only mock server (no extra deps): proves
    /// [`upload_record`] POSTs to `/api/projects/{project_id}/runs` with `Authorization: Bearer
    /// {api_key}`, serializes the [`RunRecord`] as the JSON body, and parses a `201 Created`
    /// `{"runId","deduped"}` response into an [`UploadResponse`]. This is also the sole user of the
    /// `#[cfg(test)]` [`Upload::with_base_url`] override (which retargets the otherwise-fixed
    /// `EVALFORGE_BASE_URL` at the local mock), so it keeps that helper from being dead code.
    #[test]
    fn upload_record_posts_to_evalforge_and_parses_201() {
        // Bind to an ephemeral port and read it back, so the test is self-contained and parallel-safe.
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock server");
        let addr = listener.local_addr().expect("local_addr");

        // One handler thread, one accept. It captures what the client actually sent and returns the
        // assertions out via the JoinHandle so a failed assertion can't be silently swallowed: the
        // returned tuple is `(method_path_ok, auth_ok)`, and any `.expect`/panic here surfaces on join.
        let handle = thread::spawn(move || {
            let (mut stream, _peer) = listener.accept().expect("accept connection");

            // Read until the end of the headers (`\r\n\r\n`), tolerant of partial reads.
            let mut buf: Vec<u8> = Vec::new();
            let mut chunk = [0u8; 1024];
            let header_end = loop {
                if let Some(pos) = find_subslice(&buf, b"\r\n\r\n") {
                    break pos + 4;
                }
                let n = stream.read(&mut chunk).expect("read request headers");
                if n == 0 {
                    panic!("connection closed before headers completed");
                }
                buf.extend_from_slice(&chunk[..n]);
            };

            let header_text = String::from_utf8_lossy(&buf[..header_end]).into_owned();

            // Parse Content-Length case-insensitively; default to 0 when absent.
            let content_length = header_text
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    if name.trim().eq_ignore_ascii_case("content-length") {
                        value.trim().parse::<usize>().ok()
                    } else {
                        None
                    }
                })
                .unwrap_or(0);

            // Read exactly the declared body length (some may already be buffered after the headers).
            let mut body = buf[header_end..].to_vec();
            while body.len() < content_length {
                let n = stream.read(&mut chunk).expect("read request body");
                if n == 0 {
                    break;
                }
                body.extend_from_slice(&chunk[..n]);
            }

            // Method + path: the first header line is the request line `POST /api/.../runs HTTP/1.1`.
            let request_line = header_text.lines().next().unwrap_or_default().to_owned();
            let method_path_ok = request_line.starts_with("POST ")
                && request_line.contains("/api/projects/proj-xyz/runs");

            // Auth: a `Authorization: Bearer {api_key}` header must be present (case-insensitive name).
            let auth_ok = header_text.lines().any(|line| {
                line.split_once(':')
                    .map(|(name, value)| {
                        name.trim().eq_ignore_ascii_case("authorization")
                            && value.trim() == "Bearer sk-eval-testkey"
                    })
                    .unwrap_or(false)
            });

            // Sanity: the body is the serialized RunRecord, so it must carry the model we uploaded.
            let body_text = String::from_utf8_lossy(&body);
            assert!(
                body_text.contains("\"model\":\"mock-model\""),
                "request body should be the serialized RunRecord, got: {body_text}"
            );

            // Write a valid 201 response with the dedup-shaped JSON the client parses.
            let json = r#"{"runId":"run_abc123","deduped":false}"#;
            let response = format!(
                "HTTP/1.1 201 Created\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                json.len(),
                json
            );
            stream
                .write_all(response.as_bytes())
                .expect("write response");
            stream.flush().expect("flush response");

            (method_path_ok, auth_ok)
        });

        // Point an upload at the local mock (plain http) and POST a minimal record.
        let upload = Upload::new("proj-xyz".into(), "sk-eval-testkey".into())
            .with_base_url(format!("http://{addr}"));
        let report = EvalReport::new(Vec::new(), 0.0, "local: m".into(), "sys".into());
        let record = RunRecord {
            model: "mock-model".into(),
            timestamp_display: "2026-06-18 14:03:21".into(),
            timestamp_file: "20260618-140321".into(),
            backend: "local".into(),
            cases_dir: "cases".into(),
            system_prompt: "sys".into(),
            report,
        };

        let resp = upload_record(&upload, &record).expect("upload");
        assert_eq!(resp.run_id, "run_abc123");
        assert!(!resp.deduped);

        // Now drain the server-side assertions; a panic in the thread re-raises here.
        let (method_path_ok, auth_ok) = handle.join().expect("handler thread");
        assert!(method_path_ok, "POST to /api/projects/proj-xyz/runs");
        assert!(
            auth_ok,
            "Authorization: Bearer sk-eval-testkey header present"
        );
    }

    /// First index of `needle` within `haystack`, or `None`. Small `std`-only helper for the mock above
    /// (no `memchr`/extra deps) so we can find the `\r\n\r\n` header terminator in the raw request bytes.
    fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
        if needle.is_empty() || haystack.len() < needle.len() {
            return None;
        }
        haystack
            .windows(needle.len())
            .position(|window| window == needle)
    }
}
