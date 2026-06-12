# Bundled External Producers Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship external producer helpers inside the normal Code Intelligence install bundle while keeping automatic external indexing disabled by default.

**Architecture:** Add a producer manifest as the packaging contract, teach the Rust runtime to resolve producer commands from env overrides, bundled executables, then PATH, and expose producer availability through install/status and stats APIs. Update npm and release packaging to validate and ship the bundle shape without changing the indexing default policy.

**Tech Stack:** Rust 2021, serde/serde_json, existing MCP handlers, Node.js CommonJS install scripts, GitHub Actions release tarball packaging.

---

## File Structure

- Create `producers/manifest.json`: shipped producer bundle contract used by npm validation, release packaging, and Rust status reporting.
- Create `src/external_index/manifest.rs`: typed manifest loader and availability checks. It owns manifest parsing and executable lookup metadata.
- Modify `src/external_index/mod.rs`: export the new manifest module.
- Modify `src/external_index/producers.rs`: align `ProducerSpec` with bundled helper names and use runtime command resolution.
- Modify `src/handlers/index.rs`: add producer availability to `get_index_stats`.
- Modify `src/server/api/repos.rs`: add producer availability to repo detail stats used by the dashboard.
- Modify `src/install.rs`: print producer bundle availability during `install` and `status`.
- Create `npm/bundle.js`: shared npm bundle validation helper.
- Create `npm-standalone/bundle.js`: standalone package copy of the bundle validation helper.
- Modify `npm/install.js` and `npm-standalone/install.js`: validate bundle contents after tar extraction.
- Modify `.github/workflows/release.yml`: archive server, manifest, and producer helper executables together.
- Modify `README.md`, `npm/README.md`, and `npm-standalone/README.md`: document that producers are bundled but not auto-run by default.

## Task 1: Add Producer Manifest Contract

**Files:**
- Create: `producers/manifest.json`
- Create: `src/external_index/manifest.rs`
- Modify: `src/external_index/mod.rs`
- Test: `src/external_index/manifest.rs`

- [ ] **Step 1: Write the failing manifest tests**

Add this test module to the new `src/external_index/manifest.rs` file:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_manifest_lists_every_supported_producer() {
        let manifest = bundled_manifest().expect("manifest parses");
        let ids = manifest
            .producers
            .iter()
            .map(|producer| producer.id.as_str())
            .collect::<std::collections::BTreeSet<_>>();

        assert_eq!(
            ids,
            [
                "c",
                "cpp",
                "csharp",
                "go",
                "java",
                "kotlin",
                "python",
                "ruby",
                "rust",
                "swift",
                "typescript"
            ]
            .into_iter()
            .collect::<std::collections::BTreeSet<_>>()
        );
    }

    #[test]
    fn manifest_executable_names_use_code_intelligence_prefix() {
        let manifest = bundled_manifest().expect("manifest parses");

        for producer in manifest.producers {
            assert!(
                producer.executable.starts_with("code-intelligence-external-"),
                "unexpected executable for {}: {}",
                producer.id,
                producer.executable
            );
        }
    }
}
```

- [ ] **Step 2: Run the tests and verify they fail**

Run:

```bash
cargo test external_index::manifest --lib
```

Expected: fail because `src/external_index/manifest.rs`, `ProducerManifest`, and `bundled_manifest()` do not exist yet.

- [ ] **Step 3: Create `producers/manifest.json`**

Create this exact file:

```json
{
  "schema_version": 1,
  "producers": [
    {
      "id": "typescript",
      "language": "typescript",
      "executable": "code-intelligence-external-typescript",
      "tier": "first_class",
      "output_file": "typescript-normalized.json",
      "requires_project_toolchain": true,
      "description": "TypeScript and JavaScript external index producer"
    },
    {
      "id": "rust",
      "language": "rust",
      "executable": "code-intelligence-external-rust",
      "tier": "first_class",
      "output_file": "rust-normalized.json",
      "requires_project_toolchain": true,
      "description": "Rust external index producer"
    },
    {
      "id": "python",
      "language": "python",
      "executable": "code-intelligence-external-python",
      "tier": "first_class",
      "output_file": "python-normalized.json",
      "requires_project_toolchain": true,
      "description": "Python external index producer"
    },
    {
      "id": "go",
      "language": "go",
      "executable": "code-intelligence-external-go",
      "tier": "first_class",
      "output_file": "go-normalized.json",
      "requires_project_toolchain": true,
      "description": "Go external index producer"
    },
    {
      "id": "java",
      "language": "java",
      "executable": "code-intelligence-external-java",
      "tier": "build_aware",
      "output_file": "java-normalized.json",
      "requires_project_toolchain": true,
      "description": "Java external index producer"
    },
    {
      "id": "kotlin",
      "language": "kotlin",
      "executable": "code-intelligence-external-kotlin",
      "tier": "build_aware",
      "output_file": "kotlin-normalized.json",
      "requires_project_toolchain": true,
      "description": "Kotlin external index producer"
    },
    {
      "id": "csharp",
      "language": "csharp",
      "executable": "code-intelligence-external-csharp",
      "tier": "build_aware",
      "output_file": "csharp-normalized.json",
      "requires_project_toolchain": true,
      "description": "C# external index producer"
    },
    {
      "id": "swift",
      "language": "swift",
      "executable": "code-intelligence-external-swift",
      "tier": "build_aware",
      "output_file": "swift-normalized.json",
      "requires_project_toolchain": true,
      "description": "Swift external index producer"
    },
    {
      "id": "c",
      "language": "c",
      "executable": "code-intelligence-external-c",
      "tier": "compile_database",
      "output_file": "c-normalized.json",
      "requires_project_toolchain": true,
      "description": "C external index producer"
    },
    {
      "id": "cpp",
      "language": "cpp",
      "executable": "code-intelligence-external-cpp",
      "tier": "compile_database",
      "output_file": "cpp-normalized.json",
      "requires_project_toolchain": true,
      "description": "C++ external index producer"
    },
    {
      "id": "ruby",
      "language": "ruby",
      "executable": "code-intelligence-external-ruby",
      "tier": "fallback_only",
      "output_file": "ruby-normalized.json",
      "requires_project_toolchain": false,
      "description": "Ruby fallback external index producer"
    }
  ]
}
```

- [ ] **Step 4: Implement the manifest module**

Create `src/external_index/manifest.rs` with:

```rust
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct ProducerManifest {
    pub schema_version: u32,
    pub producers: Vec<ProducerManifestEntry>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct ProducerManifestEntry {
    pub id: String,
    pub language: String,
    pub executable: String,
    pub tier: String,
    pub output_file: String,
    pub requires_project_toolchain: bool,
    pub description: String,
}

pub fn bundled_manifest() -> Result<ProducerManifest> {
    serde_json::from_str(include_str!("../../producers/manifest.json"))
        .context("Failed to parse bundled external producer manifest")
}
```

Modify `src/external_index/mod.rs`:

```rust
pub mod artifact;
pub mod importer;
pub mod manifest;
pub mod producers;
pub mod provider;
```

- [ ] **Step 5: Run the manifest tests**

Run:

```bash
cargo test external_index::manifest --lib
```

Expected: both manifest tests pass.

- [ ] **Step 6: Commit**

```bash
git add producers/manifest.json src/external_index/manifest.rs src/external_index/mod.rs
git commit -m "feat: add external producer bundle manifest"
```

## Task 2: Resolve Bundled Producer Commands At Runtime

**Files:**
- Modify: `src/external_index/producers.rs`
- Test: `src/external_index/producers.rs`

- [ ] **Step 1: Write failing resolution tests**

Add these tests to the existing `#[cfg(test)] mod tests` in `src/external_index/producers.rs`:

```rust
#[test]
fn producer_resolution_prefers_env_override() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|err| err.into_inner());
    let _env = EnvVarGuard::set(
        "EXTERNAL_INDEX_RUST_COMMAND",
        "/custom/code-intelligence-external-rust",
    );
    let temp = tempfile::tempdir().expect("tempdir");
    let spec = producer_spec_by_id("rust").expect("rust spec");

    let resolved = resolve_producer_program_for_dir(spec, Some(temp.path())).expect("resolve");

    assert_eq!(resolved.program, "/custom/code-intelligence-external-rust");
    assert_eq!(resolved.source, ProducerCommandSource::Override);
}

#[test]
fn producer_resolution_uses_bundled_executable_before_path() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|err| err.into_inner());
    std::env::remove_var("EXTERNAL_INDEX_RUST_COMMAND");
    let temp = tempfile::tempdir().expect("tempdir");
    let bundled = temp.path().join("code-intelligence-external-rust");
    std::fs::write(&bundled, "#!/bin/sh\nexit 0\n").expect("write bundled producer");
    make_executable(&bundled);
    let spec = producer_spec_by_id("rust").expect("rust spec");

    let resolved = resolve_producer_program_for_dir(spec, Some(temp.path())).expect("resolve");

    assert_eq!(resolved.program, bundled.to_string_lossy());
    assert_eq!(resolved.source, ProducerCommandSource::Bundled);
}

#[test]
fn producer_resolution_reports_missing_when_not_overridden_or_bundled() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|err| err.into_inner());
    std::env::remove_var("EXTERNAL_INDEX_RUST_COMMAND");
    let temp = tempfile::tempdir().expect("tempdir");
    let spec = producer_spec_by_id("rust").expect("rust spec");

    let resolved = resolve_producer_program_for_dir(spec, Some(temp.path()));

    assert!(resolved.is_none());
}

#[cfg(unix)]
fn make_executable(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = std::fs::metadata(path).expect("metadata").permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(path, permissions).expect("chmod");
}
```

- [ ] **Step 2: Run the tests and verify they fail**

Run:

```bash
cargo test external_index::producers::tests::producer_resolution --lib
```

Expected: fail because `producer_spec_by_id`, `resolve_producer_program_for_dir`, and `ProducerCommandSource` do not exist.

- [ ] **Step 3: Add resolution types and helper functions**

Modify `src/external_index/producers.rs` near `ProducerCommand`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProducerCommandSource {
    Override,
    Bundled,
    Path,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedProducerProgram {
    pub program: String,
    pub source: ProducerCommandSource,
}

fn producer_spec_by_id(id: &str) -> Option<ProducerSpec> {
    PRODUCER_SPECS.iter().copied().find(|spec| spec.id == id)
}

fn resolve_producer_program(spec: ProducerSpec) -> Option<ResolvedProducerProgram> {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(|parent| parent.to_path_buf()));
    resolve_producer_program_for_dir(spec, exe_dir.as_deref())
}

fn resolve_producer_program_for_dir(
    spec: ProducerSpec,
    exe_dir: Option<&std::path::Path>,
) -> Option<ResolvedProducerProgram> {
    if let Ok(program) = std::env::var(spec.command_env) {
        if !program.trim().is_empty() {
            return Some(ResolvedProducerProgram {
                program,
                source: ProducerCommandSource::Override,
            });
        }
    }

    if let Some(exe_dir) = exe_dir {
        let bundled = exe_dir.join(spec.default_program);
        if is_executable(&bundled) {
            return Some(ResolvedProducerProgram {
                program: bundled.to_string_lossy().into_owned(),
                source: ProducerCommandSource::Bundled,
            });
        }
    }

    if path_lookup(spec.default_program) {
        return Some(ResolvedProducerProgram {
            program: spec.default_program.to_string(),
            source: ProducerCommandSource::Path,
        });
    }

    None
}

fn path_lookup(program: &str) -> bool {
    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|dir| is_executable(&dir.join(program))))
        .unwrap_or(false)
}

fn is_executable(path: &std::path::Path) -> bool {
    path.is_file()
        && std::fs::metadata(path)
            .map(|metadata| {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    metadata.permissions().mode() & 0o111 != 0
                }
                #[cfg(not(unix))]
                {
                    !metadata.permissions().readonly()
                }
            })
            .unwrap_or(false)
}
```

Change the TypeScript spec default program to the bundled helper:

```rust
default_program: "code-intelligence-external-typescript",
```

- [ ] **Step 4: Run the resolution tests**

Run:

```bash
cargo test external_index::producers::tests::producer_resolution --lib
```

Expected: all three resolution tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/external_index/producers.rs
git commit -m "feat: resolve bundled external producers"
```

## Task 3: Return Source-Aware Producer Diagnostics

**Files:**
- Modify: `src/external_index/producers.rs`
- Test: `src/external_index/producers.rs`

- [ ] **Step 1: Write failing diagnostics tests**

Add:

```rust
#[test]
fn generate_reports_missing_bundle_when_resolved_command_is_absent() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|err| err.into_inner());
    std::env::remove_var("EXTERNAL_INDEX_RUST_COMMAND");
    let store = SqliteStore::open_in_memory().expect("sqlite");
    store.init().expect("init");
    let repo = tempfile::tempdir().expect("repo");
    std::fs::write(repo.path().join("Cargo.toml"), "[package]\nname = \"demo\"\n")
        .expect("write Cargo.toml");
    let repo_data = tempfile::tempdir().expect("repo data");

    let response = generate_and_import(
        &store,
        repo.path().to_str().expect("utf8"),
        Utf8Path::from_path(repo_data.path()).expect("utf8"),
        None,
        None,
    )
    .expect("response");

    assert_eq!(response["ok"], false);
    assert_eq!(response["status"], "missing_bundle");
    assert_eq!(response["producer"], "rust");
    assert_eq!(response["program"], "code-intelligence-external-rust");
    assert_eq!(response["command_source"], "missing");
}
```

- [ ] **Step 2: Run the diagnostics test and verify it fails**

Run:

```bash
cargo test external_index::producers::tests::generate_reports_missing_bundle_when_resolved_command_is_absent --lib
```

Expected: fail because current code reports `missing_toolchain` only after `Command::new` fails.

- [ ] **Step 3: Update `generate_with_spec` to use resolution**

Replace the current `program` assignment in `generate_with_spec` with:

```rust
let resolved = match resolve_producer_program(spec) {
    Some(resolved) => resolved,
    None => {
        return Ok(json!({
            "ok": false,
            "status": "missing_bundle",
            "producer": spec.id,
            "language": language,
            "program": spec.default_program,
            "command_source": "missing",
            "supported_producers": supported_producers(),
        }));
    }
};
let command = producer_command(&resolved.program, repo_root, output_path.as_str());
let command_source = command_source_str(resolved.source);
```

Add:

```rust
fn command_source_str(source: ProducerCommandSource) -> &'static str {
    match source {
        ProducerCommandSource::Override => "override",
        ProducerCommandSource::Bundled => "bundled",
        ProducerCommandSource::Path => "path",
    }
}
```

Add `"command_source": command_source` to success, `producer_failed`, `artifact_missing`, and import success JSON responses. For `ErrorKind::NotFound`, keep a defensive response:

```rust
"status": "missing_bundle",
"command_source": command_source,
```

- [ ] **Step 4: Run affected producer tests**

Run:

```bash
cargo test external_index::producers --lib
```

Expected: all producer tests pass. Existing tests that expected `missing_toolchain` for missing commands must be updated to `missing_bundle` only when no override/bundled/PATH command exists. Tests using env overrides with intentionally missing absolute command paths should continue to report `missing_bundle` from the defensive `Command::new` branch and include `command_source: "override"`.

- [ ] **Step 5: Commit**

```bash
git add src/external_index/producers.rs
git commit -m "feat: report external producer command source"
```

## Task 4: Add Producer Availability To Stats APIs

**Files:**
- Modify: `src/external_index/manifest.rs`
- Modify: `src/handlers/index.rs`
- Modify: `src/server/api/repos.rs`
- Modify: `ui/src/api/types.ts`
- Modify: `ui/src/features/repos/ReposView.tsx`
- Test: `src/handlers/index.rs`, `src/server/api/repos.rs`, `ui/src/api/repos.test.ts`

- [ ] **Step 1: Write failing Rust availability tests**

In `src/external_index/manifest.rs`, add:

```rust
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ProducerAvailability {
    pub id: String,
    pub language: String,
    pub tier: String,
    pub executable: String,
    pub availability: String,
}
```

Add a failing test:

```rust
#[test]
fn producer_availability_marks_missing_when_executable_absent() {
    let temp = tempfile::tempdir().expect("tempdir");
    let availability = producer_availability_for_dir(Some(temp.path())).expect("availability");
    let rust = availability
        .iter()
        .find(|producer| producer.id == "rust")
        .expect("rust producer");

    assert_eq!(rust.availability, "missing");
    assert_eq!(rust.executable, "code-intelligence-external-rust");
}
```

- [ ] **Step 2: Implement availability calculation**

Add to `src/external_index/manifest.rs`:

```rust
pub fn producer_availability() -> Result<Vec<ProducerAvailability>> {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(|parent| parent.to_path_buf()));
    producer_availability_for_dir(exe_dir.as_deref())
}

pub fn producer_availability_for_dir(
    exe_dir: Option<&std::path::Path>,
) -> Result<Vec<ProducerAvailability>> {
    let manifest = bundled_manifest()?;
    Ok(manifest
        .producers
        .into_iter()
        .map(|producer| {
            let bundled_path = exe_dir.map(|dir| dir.join(&producer.executable));
            let availability = if bundled_path
                .as_deref()
                .map(super::producers::is_executable)
                .unwrap_or(false)
            {
                "bundled"
            } else {
                "missing"
            };
            ProducerAvailability {
                id: producer.id,
                language: producer.language,
                tier: producer.tier,
                executable: bundled_path
                    .map(|path| path.to_string_lossy().into_owned())
                    .unwrap_or(producer.executable),
                availability: availability.to_string(),
            }
        })
        .collect())
}
```

Make `is_executable` in `src/external_index/producers.rs` `pub(crate)` so the manifest module can reuse it:

```rust
pub(crate) fn is_executable(path: &std::path::Path) -> bool {
```

- [ ] **Step 3: Add stats JSON**

In `src/handlers/index.rs`, compute:

```rust
let external_producers = crate::external_index::manifest::producer_availability()
    .unwrap_or_else(|_| Vec::new());
```

Add to the JSON object:

```rust
"external_producers": external_producers,
```

In `src/server/api/repos.rs`, add the same `external_producers` field to `read_repo_stats`.

- [ ] **Step 4: Update UI types and compact display**

In `ui/src/api/types.ts`, add:

```ts
external_producers: Array<{
  id: string;
  language: string;
  tier: string;
  executable: string;
  availability: string;
}>;
```

In `ui/src/features/repos/ReposView.tsx`, add compact fields near external index stats:

```tsx
<Field
  label="producers"
  value={`${detail.data.stats.external_producers.filter((producer) => producer.availability !== "missing").length}/${detail.data.stats.external_producers.length}`}
/>
```

- [ ] **Step 5: Update API tests**

In `ui/src/api/repos.test.ts`, extend the repo detail payload with:

```ts
external_producers: [
  {
    id: "rust",
    language: "rust",
    tier: "first_class",
    executable: "/bin/code-intelligence-external-rust",
    availability: "bundled",
  },
],
```

Assert:

```ts
expect(result.stats?.external_producers?.[0]?.availability).toBe("bundled");
```

- [ ] **Step 6: Run focused checks**

Run:

```bash
cargo test external_index::manifest --lib
cargo test server::api::repos::tests::read_repo_stats_includes_external_overlay_counts --lib
/Users/dikrana/.cache/codex-runtimes/codex-primary-runtime/dependencies/node/bin/node ui/node_modules/typescript/bin/tsc --noEmit -p ui/tsconfig.json
```

Expected: Rust tests and TypeScript check pass.

- [ ] **Step 7: Commit**

```bash
git add src/external_index/manifest.rs src/external_index/producers.rs src/handlers/index.rs src/server/api/repos.rs ui/src/api/types.ts ui/src/api/repos.test.ts ui/src/features/repos/ReposView.tsx
git commit -m "feat: expose external producer availability"
```

## Task 5: Print Producer Bundle Summary During Install And Status

**Files:**
- Modify: `src/install.rs`
- Test: `src/install.rs`

- [ ] **Step 1: Write failing summary unit test**

Add to the existing `#[cfg(test)]` module in `src/install.rs`:

```rust
#[test]
fn producer_summary_counts_available_and_missing() {
    let producers = vec![
        crate::external_index::manifest::ProducerAvailability {
            id: "rust".to_string(),
            language: "rust".to_string(),
            tier: "first_class".to_string(),
            executable: "/bin/code-intelligence-external-rust".to_string(),
            availability: "bundled".to_string(),
        },
        crate::external_index::manifest::ProducerAvailability {
            id: "python".to_string(),
            language: "python".to_string(),
            tier: "first_class".to_string(),
            executable: "code-intelligence-external-python".to_string(),
            availability: "missing".to_string(),
        },
    ];

    assert_eq!(
        render_producer_summary(&producers, false),
        "External producers: bundled 1/2, auto indexing disabled"
    );
}
```

- [ ] **Step 2: Implement summary renderer**

Add near install helper functions:

```rust
fn render_producer_summary(
    producers: &[crate::external_index::manifest::ProducerAvailability],
    auto_enabled: bool,
) -> String {
    let available = producers
        .iter()
        .filter(|producer| producer.availability != "missing")
        .count();
    let policy = if auto_enabled {
        "auto indexing enabled"
    } else {
        "auto indexing disabled"
    };
    format!(
        "External producers: bundled {available}/{}, {policy}",
        producers.len()
    )
}
```

In `handle_install`, after daemon registration and before config patch prompt, add:

```rust
let producers = crate::external_index::manifest::producer_availability().unwrap_or_default();
println!("{}", render_producer_summary(&producers, false));
```

In `handle_status`, add the same summary using `StandaloneConfig::load(None, None, None).map(|cfg| cfg.external_index_auto).unwrap_or(false)` for `auto_enabled`.

- [ ] **Step 3: Run install tests**

Run:

```bash
cargo test install::tests::producer_summary_counts_available_and_missing --lib
```

Expected: pass.

- [ ] **Step 4: Commit**

```bash
git add src/install.rs
git commit -m "feat: show external producer bundle status"
```

## Task 6: Validate Bundle Contents In npm Installers

**Files:**
- Create: `npm/bundle.js`
- Modify: `npm/install.js`
- Modify: `npm-standalone/install.js`
- Test: `npm/bundle.test.js`

- [ ] **Step 1: Write failing Node tests**

Create `npm/bundle.test.js`:

```js
"use strict";

const assert = require("node:assert");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const test = require("node:test");
const { validateBundle } = require("./bundle");

test("validateBundle accepts server binary, manifest, and executable producers", () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "ci-bundle-"));
  try {
    fs.writeFileSync(path.join(dir, "code-intelligence-mcp-server"), "");
    fs.chmodSync(path.join(dir, "code-intelligence-mcp-server"), 0o755);
    fs.mkdirSync(path.join(dir, "producers"));
    fs.writeFileSync(
      path.join(dir, "producers", "manifest.json"),
      JSON.stringify({
        producers: [
          {
            executable: "code-intelligence-external-rust",
          },
        ],
      }),
    );
    fs.writeFileSync(path.join(dir, "code-intelligence-external-rust"), "");
    fs.chmodSync(path.join(dir, "code-intelligence-external-rust"), 0o755);

    assert.deepEqual(validateBundle(dir).missing, []);
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
});

test("validateBundle reports missing producer executables", () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "ci-bundle-"));
  try {
    fs.writeFileSync(path.join(dir, "code-intelligence-mcp-server"), "");
    fs.chmodSync(path.join(dir, "code-intelligence-mcp-server"), 0o755);
    fs.mkdirSync(path.join(dir, "producers"));
    fs.writeFileSync(
      path.join(dir, "producers", "manifest.json"),
      JSON.stringify({
        producers: [
          {
            executable: "code-intelligence-external-rust",
          },
        ],
      }),
    );

    assert.deepEqual(validateBundle(dir).missing, ["code-intelligence-external-rust"]);
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
});
```

- [ ] **Step 2: Run Node tests and verify they fail**

Run:

```bash
/Users/dikrana/.cache/codex-runtimes/codex-primary-runtime/dependencies/node/bin/node --test npm/bundle.test.js
```

Expected: fail because `npm/bundle.js` does not exist.

- [ ] **Step 3: Implement `npm/bundle.js`**

Create:

```js
"use strict";

const fs = require("node:fs");
const path = require("node:path");

const SERVER_BINARY = "code-intelligence-mcp-server";

function isExecutable(filePath) {
  try {
    fs.accessSync(filePath, fs.constants.X_OK);
    return true;
  } catch {
    return false;
  }
}

function readManifest(binDir) {
  const manifestPath = path.join(binDir, "producers", "manifest.json");
  return JSON.parse(fs.readFileSync(manifestPath, "utf8"));
}

function validateBundle(binDir) {
  const missing = [];
  if (!isExecutable(path.join(binDir, SERVER_BINARY))) {
    missing.push(SERVER_BINARY);
  }

  const manifest = readManifest(binDir);
  for (const producer of manifest.producers || []) {
    if (!isExecutable(path.join(binDir, producer.executable))) {
      missing.push(producer.executable);
    }
  }

  return { missing };
}

module.exports = {
  validateBundle,
};
```

- [ ] **Step 4: Use validation in both install scripts**

In `npm/install.js`, add:

```js
const { validateBundle } = require("./bundle");
```

After extraction, replace server-only verification with:

```js
const validation = validateBundle(binDir);
if (validation.missing.length === 0) {
  fs.chmodSync(destBinary, 0o755);
  console.log(`Successfully installed to ${destBinary}`);
} else {
  console.error("Extraction failed: bundle is incomplete.");
  console.error(`Missing: ${validation.missing.join(", ")}`);
  console.log("Contents of bin directory:", fs.readdirSync(binDir));
  process.exit(1);
}
```

Apply the same change to `npm-standalone/install.js`, requiring `../npm/bundle` is not safe after package publication, so copy `bundle.js` into `npm-standalone/bundle.js` or create a separate identical file there. Prefer creating `npm-standalone/bundle.js` with the same contents so each package is self-contained.

- [ ] **Step 5: Run Node tests**

Run:

```bash
/Users/dikrana/.cache/codex-runtimes/codex-primary-runtime/dependencies/node/bin/node --test npm/bundle.test.js
```

Expected: both tests pass.

- [ ] **Step 6: Commit**

```bash
git add npm/bundle.js npm/bundle.test.js npm/install.js npm-standalone/bundle.js npm-standalone/install.js
git commit -m "feat: validate npm external producer bundle"
```

## Task 7: Add Producer Helper Entry Points And Release Packaging

**Files:**
- Create: `producers/bin/code-intelligence-external-typescript`
- Create: `producers/bin/code-intelligence-external-rust`
- Create: `producers/bin/code-intelligence-external-python`
- Create: `producers/bin/code-intelligence-external-go`
- Create: `producers/bin/code-intelligence-external-java`
- Create: `producers/bin/code-intelligence-external-kotlin`
- Create: `producers/bin/code-intelligence-external-csharp`
- Create: `producers/bin/code-intelligence-external-swift`
- Create: `producers/bin/code-intelligence-external-c`
- Create: `producers/bin/code-intelligence-external-cpp`
- Create: `producers/bin/code-intelligence-external-ruby`
- Modify: `.github/workflows/release.yml`
- Test: shell syntax check for producer entrypoints

- [ ] **Step 1: Create helper scripts**

Each helper must be executable and accept the existing contract:

```text
index --output /abs/path/to/<language>-normalized.json
```

For this shipping phase, create wrappers that fail with clear diagnostics until the language-specific generator is implemented or delegated. Example for `producers/bin/code-intelligence-external-rust`:

```sh
#!/bin/sh
set -eu

if [ "${1:-}" != "index" ]; then
  echo "usage: code-intelligence-external-rust index --output <normalized-json>" >&2
  exit 64
fi

echo "code-intelligence-external-rust is bundled but no Rust generator is enabled yet" >&2
exit 69
```

Use the same shape for all producers, replacing the executable name and language. This preserves the owned entrypoint contract without pretending compiler-grade generation exists before implementation and benchmark.

- [ ] **Step 2: Make helpers executable**

Run:

```bash
chmod +x producers/bin/code-intelligence-external-*
```

- [ ] **Step 3: Check shell syntax**

Run:

```bash
for f in producers/bin/code-intelligence-external-*; do sh -n "$f"; done
```

Expected: no output and exit code 0.

- [ ] **Step 4: Update release archive step**

Change `.github/workflows/release.yml` archive step to:

```yaml
      - name: Archive Binary Bundle
        shell: bash
        run: |
          mkdir -p bundle/producers
          cp target/aarch64-apple-darwin/release/code-intelligence-mcp-server bundle/
          cp producers/manifest.json bundle/producers/manifest.json
          cp producers/bin/code-intelligence-external-* bundle/
          chmod 755 bundle/code-intelligence-mcp-server bundle/code-intelligence-external-*
          tar czf code-intelligence-mcp-server-aarch64-apple-darwin.tar.gz -C bundle .
```

- [ ] **Step 5: Commit**

```bash
git add producers/bin .github/workflows/release.yml
git commit -m "feat: bundle external producer entrypoints"
```

## Task 8: Document Shipping Policy

**Files:**
- Modify: `README.md`
- Modify: `npm/README.md`
- Modify: `npm-standalone/README.md`
- Test: docs grep

- [ ] **Step 1: Add documentation section**

Add this section near existing external index or configuration docs:

```markdown
### Bundled External Producers

Code Intelligence installs external producer entrypoints with the server binary. These helpers are resolved from the installed binary directory first, then from `PATH`, with `EXTERNAL_INDEX_<LANG>_COMMAND` still available as an explicit override.

Bundled producers do not make external indexing automatic. The default remains native Tree-sitter indexing:

```bash
EXTERNAL_INDEX_AUTO=false
EXTERNAL_INDEX_ON_REFRESH=disabled
```

Use `generate_external_index` or opt-in refresh configuration to run producers before benchmark-proven defaults are enabled.
```

- [ ] **Step 2: Verify docs mention the policy**

Run:

```bash
rg "Bundled External Producers|EXTERNAL_INDEX_AUTO=false|EXTERNAL_INDEX_ON_REFRESH=disabled" README.md npm/README.md npm-standalone/README.md
```

Expected: each file contains the section or equivalent policy text.

- [ ] **Step 3: Commit**

```bash
git add README.md npm/README.md npm-standalone/README.md
git commit -m "docs: document bundled external producer policy"
```

## Task 9: Final Verification Before Benchmark

**Files:**
- Verify all changed files.

- [ ] **Step 1: Run Rust formatting**

Run:

```bash
cargo fmt --check
```

Expected: exit code 0.

- [ ] **Step 2: Run Rust tests**

Run:

```bash
cargo test
```

Expected: all non-ignored tests pass.

- [ ] **Step 3: Run hash-backend Rust tests**

Run:

```bash
EMBEDDINGS_BACKEND=hash cargo test
```

Expected: all non-ignored tests pass.

- [ ] **Step 4: Run UI typecheck**

Run:

```bash
/Users/dikrana/.cache/codex-runtimes/codex-primary-runtime/dependencies/node/bin/node ui/node_modules/typescript/bin/tsc --noEmit -p ui/tsconfig.json
```

Expected: exit code 0.

- [ ] **Step 5: Run npm bundle tests**

Run:

```bash
/Users/dikrana/.cache/codex-runtimes/codex-primary-runtime/dependencies/node/bin/node --test npm/bundle.test.js
```

Expected: all tests pass.

- [ ] **Step 6: Confirm clean worktree**

Run:

```bash
git status --short
```

Expected: no output.

This is the handoff point for benchmark work. Do not enable automatic external indexing by default before benchmark results are reviewed.
