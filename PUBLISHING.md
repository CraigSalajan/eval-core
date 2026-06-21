# Publishing & extraction checklist for `eval-core`

This crate currently lives inside the AetherCore Cargo workspace but is written to be
**game-agnostic and standalone** (zero AetherCore/Bevy coupling). It is prepped to publish to
crates.io *in place* now, and to be physically extracted into its own repository later.

The crate name `eval-core` is reserved/available on crates.io. License is `MIT OR Apache-2.0`
(see `LICENSE-MIT` and `LICENSE-APACHE`). MSRV is **Rust 1.85** (edition 2024).

---

## Automated releases (current process — CI publishes on tag)

CI now publishes to crates.io automatically when a semver tag is pushed. The workflow is
`.github/workflows/release.yml`; it authenticates with **crates.io Trusted Publishing** (GitHub
OIDC), so there is **no `CARGO_REGISTRY_TOKEN` secret** stored in the repo.

### One-time setup (do this once, before the first automated release)
On crates.io: open the `eval-core` crate → **Settings → Trusted Publishing → Add a GitHub
publisher**:

| Field             | Value           |
| ----------------- | --------------- |
| Repository owner  | `CraigSalajan`  |
| Repository name   | `eval-core`     |
| Workflow filename | `release.yml`   |
| Environment       | *(leave blank)* |

Until this entry exists, the publish step fails with an authentication error.

### Cutting a release
1. Bump `version` in `Cargo.toml` and merge it to `main` via PR (`main` is now protected).
2. Tag the merged commit with the **same** version and push the tag:
   ```sh
   git tag v0.4.0
   git push origin v0.4.0
   ```
   (tag pattern: `vX.Y.Z`.)
3. `release.yml` then verifies the tag matches `Cargo.toml`, runs the tests, publishes to
   crates.io via Trusted Publishing, and cuts a GitHub Release with auto-generated notes.

The manual `cargo publish` flow in section A below is now only a fallback.

---

## A. First publish (from within the AetherCore workspace — works as-is)

Publishing from inside the workspace is fine: when `cargo publish`/`cargo package` builds the
`.crate`, Cargo **de-inherits** the `[workspace.package]` fields (`version`, `edition`, `license`,
…) and every `{ workspace = true }` dependency into the packaged `Cargo.toml`, rewriting them to
concrete values. The uploaded manifest is fully self-contained; consumers never see the workspace.
You do NOT need to extract the crate first to publish it.

Steps:

1. **Create the public GitHub repo** (e.g. `github.com/<you>/eval-core`).
2. **Set the real URLs in `crates/eval-core/Cargo.toml`** — replace the placeholders and remove the
   `TODO(publish)` comments on:
   - `repository = "..."`
   - `homepage = "..."`
   - (`documentation = "https://docs.rs/eval-core"` is already correct — leave it.)
   > These land in immutable crates.io metadata. **You cannot change them after publishing a given
   > version** — only by publishing a new version. Get them right first.
3. **Enable publishing:** delete the `publish = false` line.
4. **`cargo login`** with a crates.io API token (https://crates.io/me).
5. **`cargo package --list -p eval-core`** and confirm the file list includes:
   - `baseline/arithmetic.ron`, `baseline/language.ron`, `baseline/tool_use.ron`
   - `src/**`, `README.md`, `LICENSE-MIT`, `LICENSE-APACHE`
   - `examples/calculator.rs`, `examples/minimal.rs`
   - and does NOT include `.github/` or `PUBLISHING.md` (excluded).
6. **`cargo publish --dry-run -p eval-core`** — full build + packaging without uploading.
7. **`cargo publish -p eval-core`** — the real upload. (This version number is permanent; yanking
   hides but never deletes it.)

After publish, docs.rs builds the rustdoc automatically at https://docs.rs/eval-core.

---

## B. Physical repo extraction (when you move the directory out)

When you move `crates/eval-core/` into its own repo, it loses the workspace it was inheriting from,
so **every workspace inheritance must be replaced with a concrete value** in `Cargo.toml`. Replace:

### `[package]` inherited fields (`*.workspace = true`)
Grab the values from the root `AetherCore/Cargo.toml [workspace.package]`:

| Field          | Concrete value to inline                                |
| -------------- | ------------------------------------------------------- |
| `version`      | `version = "0.1.0"`                                     |
| `edition`      | `edition = "2024"`                                      |
| `license`      | `license = "MIT OR Apache-2.0"`                         |
| `authors`      | `authors = ["Craig Salajan <craigsalajan@gmail.com>"]` (already concrete here — keep it) |
| `rust-version` | `rust-version = "1.85"` (already concrete here — keep it) |

### `[lints]`
Replace `[lints]\nworkspace = true` with concrete lint tables mirroring the workspace
(`AetherCore/Cargo.toml [workspace.lints.*]`):

```toml
[lints.rust]
unsafe_code = "warn"
missing_debug_implementations = "warn"

[lints.clippy]
all = { level = "warn", priority = -1 }
```

### Dependencies — pin each `{ workspace = true }` to a concrete version
Take the **exact versions** from the root `AetherCore/Cargo.toml [workspace.dependencies]` (the
values below were read from there; `Cargo.lock` resolves them to the patch versions shown in
parentheses for spot-checking). Match the workspace's feature flags too.

`[dependencies]`:

| Crate         | Workspace requirement                       | Resolved (Cargo.lock) |
| ------------- | ------------------------------------------- | --------------------- |
| `serde`       | `{ version = "1", features = ["derive"] }`  | 1.0.228               |
| `serde_json`  | `"1"`                                        | 1.0.150               |
| `ron`         | `"0.8"`                                       | 0.8.1                 |
| `anyhow`      | `"1"`                                         | 1.0.102               |
| `thiserror`   | `"2"`                                         | 2.0.18                |
| `regex`       | `"1"`                                         | 1.12.4                |
| `tracing`     | `"0.1"`                                       | 0.1.44                |
| `include_dir` | `"0.7"`                                       | 0.7.4                 |

`[dev-dependencies]`:

| Crate      | Workspace requirement | Resolved (Cargo.lock) |
| ---------- | --------------------- | --------------------- |
| `tempfile` | `"3"`                  | 3.27.0                |

> Use the **caret/major requirements** from `[workspace.dependencies]` (e.g. `serde = "1"`), not
> the pinned patch versions, unless you have a reason to be stricter. The Cargo.lock column is only
> for verifying you copied the right crate. After extracting, run `cargo update` + `cargo test` to
> regenerate a standalone lockfile and confirm nothing broke.

### Files that move with the crate
`src/`, `baseline/`, `examples/`, `README.md`, `LICENSE-MIT`, `LICENSE-APACHE`, `.gitignore`,
`PUBLISHING.md`, and `.github/workflows/ci.yml` (already at the crate root so it becomes the new
repo's CI with no path changes). Also remove the crate from the AetherCore workspace `members` list
and `[workspace.dependencies]` once extracted.

---

## C. Semver / API stability

The public types (`EvalError`, `CaseOutcome`, `RunArtifacts`, run/report metadata structs) are
`#[non_exhaustive]` and are built via constructors + chainable `with_*` builders rather than struct
literals. This means **new fields/variants can be added without a breaking (major) release** —
additive changes are minor-version-compatible. Reserve major bumps for genuine breaking changes
(removing/renaming public items, changing signatures, tightening behavior).
