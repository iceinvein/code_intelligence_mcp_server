//! Resolving a git worktree to the repository it was created from, and seeding
//! the worktree's index from that base repository's index.
//!
//! A linked worktree has `.git` as a FILE containing a `gitdir:` pointer, and
//! its "common directory" is the main repository's `.git`. That common
//! directory is the authoritative link; the older approach of splitting the
//! pointer text on `/.git/worktrees/` breaks for nested worktrees and for
//! non-default worktree layouts.
//!
//! Seeding is worthwhile because a worktree's content is nearly identical to
//! its base, so copying the base's index turns the worktree's first pass into a
//! delta instead of a full parse-and-embed. It is safe because every index key
//! is a path relative to the repo root; the only absolute values are
//! `repositories.root_path`, its SHA-derived `repositories.id`, and
//! `packages.manifest_path`.

use crate::path::{Utf8Path, Utf8PathBuf};
use anyhow::{bail, Context, Result};

const DB_FILE: &str = "code-intelligence.db";
const TANTIVY_DIR: &str = "tantivy-index";
const VECTORS_DIR: &str = "vectors";

/// Return the main repository root when `path` is a linked git worktree.
///
/// Returns `None` when `path` is not a git repository, is the main repository
/// itself, or when the resolved base is not a readable directory distinct from
/// `path`.
pub fn resolve_base_repo(path: &Utf8Path) -> Option<Utf8PathBuf> {
    let repo = git2::Repository::open(path.as_std_path()).ok()?;

    // Authoritative predicate: libgit2 knows whether this is a linked worktree.
    if !repo.is_worktree() {
        return None;
    }

    // For a linked worktree, `path()` is the worktree's own private git dir
    // (`<base>/.git/worktrees/<name>`), which contains a `commondir` file
    // holding the path to the shared git dir, normally the relative `../..`.
    // This is git's documented plumbing; `Repository::commondir()` would give
    // the same answer but only landed in git2 0.20, and this project pins 0.19.
    let git_dir = repo.path();
    let pointer = std::fs::read_to_string(git_dir.join("commondir")).ok()?;
    let pointer = pointer.trim();
    if pointer.is_empty() {
        return None;
    }
    let common = if std::path::Path::new(pointer).is_absolute() {
        std::path::PathBuf::from(pointer)
    } else {
        git_dir.join(pointer)
    };

    // Canonicalize to collapse the `../..` before taking the parent, otherwise
    // `parent()` would just strip one `..` component.
    let common = std::fs::canonicalize(common).ok()?;

    // `common` is `<base>/.git`, so the base root is its parent.
    let base_root = common.parent()?;
    if !base_root.is_dir() {
        return None;
    }

    // Guard against a degenerate layout resolving back to the worktree itself.
    let canonical_self = std::fs::canonicalize(path.as_std_path()).ok()?;
    if base_root == canonical_self {
        return None;
    }

    Utf8PathBuf::from_path_buf(base_root.to_path_buf()).ok()
}

/// Everything `seed_index_from_base` needs, resolved up front.
#[derive(Debug, Clone)]
pub struct SeedPlan {
    pub base_repo_path: Utf8PathBuf,
    pub base_repo_id: String,
    pub base_data_dir: Utf8PathBuf,
    pub worktree_path: Utf8PathBuf,
    pub worktree_data_dir: Utf8PathBuf,
}

/// True when a data directory already holds index state. Seeding over such a
/// directory is unsafe, so callers treat this as "not seedable".
pub fn data_dir_has_index_artifacts(data_dir: &Utf8Path) -> bool {
    data_dir.join(DB_FILE).as_std_path().exists()
        || data_dir.join(TANTIVY_DIR).as_std_path().exists()
        || data_dir.join(VECTORS_DIR).as_std_path().exists()
}

/// Populate a worktree's data directory from its base repo's index.
///
/// The SQLite copy uses `VACUUM INTO`, which takes a consistent snapshot under
/// WAL without quiescing the base and produces a compact, WAL-free database.
/// `index_runs` is then cleared so `has_persisted_index_run` reports false and
/// the normal InitialBind lifecycle runs exactly one pass over the worktree.
///
/// Every failure path returns `Err` so the caller can delete the partial
/// directory and fall back to a full index. A half-seeded directory is never
/// reported as a success.
pub fn seed_index_from_base(plan: &SeedPlan) -> Result<()> {
    if data_dir_has_index_artifacts(&plan.worktree_data_dir) {
        bail!(
            "refusing to seed into a data directory that already holds index artifacts: {}",
            plan.worktree_data_dir
        );
    }
    std::fs::create_dir_all(plan.worktree_data_dir.as_std_path())
        .with_context(|| format!("Failed to create {}", plan.worktree_data_dir))?;

    let src_db = plan.base_data_dir.join(DB_FILE);
    if !src_db.as_std_path().is_file() {
        bail!("base index has no database at {src_db}");
    }
    let dst_db = plan.worktree_data_dir.join(DB_FILE);

    // A separate connection is deliberate: it reads the base through WAL
    // without contending on the live store's write mutex.
    let src_conn = rusqlite::Connection::open(src_db.as_std_path())
        .with_context(|| format!("Failed to open base index at {src_db}"))?;
    // VACUUM cannot take a bound parameter, so the path is inlined. Single
    // quotes are doubled per SQL string-literal escaping.
    let escaped = dst_db.as_str().replace('\'', "''");
    src_conn
        .execute_batch(&format!("VACUUM INTO '{escaped}'"))
        .with_context(|| format!("Failed to snapshot base index into {dst_db}"))?;
    drop(src_conn);

    for dir in [TANTIVY_DIR, VECTORS_DIR] {
        let src = plan.base_data_dir.join(dir);
        if src.as_std_path().exists() {
            clone_tree(&src, &plan.worktree_data_dir.join(dir))?;
        }
    }

    rewrite_seeded_db(plan, &dst_db)?;
    validate_seeded_index(&plan.worktree_data_dir)?;
    Ok(())
}

/// Open the cloned index the way the daemon will, so a copy torn by a
/// concurrent watcher pass on the base fails here (where the caller falls back
/// to a full index) rather than at query time.
///
/// Tantivy is opened through `open_readonly` rather than `open_or_create`: the
/// latter silently recreates an index it cannot open, which would turn a torn
/// clone into a quietly empty one. `open_readonly` also parses the schema and
/// builds a reader, so a `meta.json` pointing at segment files that did not
/// come across is caught here too.
fn validate_seeded_index(data_dir: &Utf8Path) -> Result<()> {
    let store = crate::storage::sqlite::SqliteStore::open(&data_dir.join(DB_FILE))
        .context("Seeded index database will not open")?;
    store
        .init()
        .context("Seeded index schema will not initialize")?;
    let symbols = store
        .count_symbols()
        .context("Seeded index symbols table is unreadable")?;
    let fingerprints: i64 = {
        let conn = store
            .read()
            .context("Seeded index will not hand out a read connection")?;
        conn.query_row("SELECT COUNT(*) FROM file_fingerprints", [], |row| {
            row.get(0)
        })
        .context("Seeded index file_fingerprints table is unreadable")?
    };

    let tantivy_dir = data_dir.join(TANTIVY_DIR);
    let vectors_dir = data_dir.join(VECTORS_DIR);

    // A seeded database that already claims files are indexed is only usable if
    // the search indexes came with it, and nothing downstream repairs the gap:
    // `TantivyIndex::open_or_create` reads a missing directory as a fresh index
    // rather than a wiped one, so `was_recreated` stays false, the seeded
    // fingerprints are never cleared, and the worktree's first pass skips every
    // file as unchanged. That would leave SQLite populated and search
    // permanently empty. Failing the seed instead lets the caller fall back to a
    // full index.
    if symbols > 0 || fingerprints > 0 {
        for dir in [&tantivy_dir, &vectors_dir] {
            if !dir.as_std_path().exists() {
                bail!(
                    "seeded index claims {fingerprints} indexed files and {symbols} symbols \
                     but has no {dir}, so its search index could never be built"
                );
            }
        }
    }

    if tantivy_dir.as_std_path().exists() {
        crate::storage::tantivy::TantivyIndex::open_readonly(&tantivy_dir)
            .context("Seeded Tantivy index will not open")?;
    }
    Ok(())
}

/// Copy a directory tree, preferring an APFS copy-on-write clone.
///
/// `clonefile(2)` is recursive and near-instant on APFS, and the copy costs no
/// disk until the two trees diverge. It requires that `dst` not already exist.
fn clone_tree(src: &Utf8Path, dst: &Utf8Path) -> Result<()> {
    use std::ffi::CString;

    let src_c = CString::new(src.as_str()).context("Source path contains a NUL byte")?;
    let dst_c = CString::new(dst.as_str()).context("Destination path contains a NUL byte")?;
    // SAFETY: both pointers come from CStrings that live until the end of this
    // function, so they outlive the call, and the flags argument of 0 is the
    // documented default.
    let rc = unsafe { libc::clonefile(src_c.as_ptr(), dst_c.as_ptr(), 0) };
    if rc == 0 {
        return Ok(());
    }

    let err = std::io::Error::last_os_error();
    tracing::warn!(
        src = %src,
        dst = %dst,
        error = %err,
        "clonefile failed, falling back to a full recursive copy"
    );
    copy_tree_recursive(src.as_std_path(), dst.as_std_path())
        .with_context(|| format!("Failed to copy {src} to {dst}"))
}

fn copy_tree_recursive(src: &std::path::Path, dst: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let target = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_tree_recursive(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

/// Retarget the copied database at the worktree.
///
/// Only three things in the index are absolute: `repositories.root_path`, the
/// SHA-256-of-root-path `repositories.id`, and `packages.manifest_path`.
/// Everything else, symbols included, is keyed on paths relative to the repo
/// root and needs no rewriting.
fn rewrite_seeded_db(plan: &SeedPlan, dst_db: &Utf8Path) -> Result<()> {
    let conn = rusqlite::Connection::open(dst_db.as_std_path())
        .with_context(|| format!("Failed to open seeded index at {dst_db}"))?;
    let tx = conn
        .unchecked_transaction()
        .context("Failed to open seed rewrite transaction")?;

    // `packages.repository_id` references `repositories(id)` with no ON UPDATE
    // clause, so the id rewrite momentarily has no valid parent whichever row
    // moves first. Deferring pushes the check to COMMIT, by which point both
    // sides agree. A no-op when foreign keys are not enforced on this
    // connection, and it resets itself at the end of the transaction.
    tx.execute_batch("PRAGMA defer_foreign_keys = ON")
        .context("Failed to defer foreign keys for the seed rewrite")?;

    tx.execute("DELETE FROM index_runs", [])
        .context("Failed to clear index_runs on the seeded index")?;

    let base_prefix = plan
        .base_repo_path
        .as_str()
        .trim_end_matches('/')
        .to_string();
    let wt_prefix = plan
        .worktree_path
        .as_str()
        .trim_end_matches('/')
        .to_string();

    let rows: Vec<(String, String)> = {
        let mut stmt = tx.prepare("SELECT id, root_path FROM repositories")?;
        let mapped = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        mapped.collect::<rusqlite::Result<Vec<_>>>()?
    };

    for (old_id, old_root) in rows {
        // git2's `workdir()` yields a trailing separator, so roots reach the
        // database in that form. Compare on the trimmed form to accept both.
        let trimmed = old_root.trim_end_matches('/');
        let Some(suffix) = trimmed.strip_prefix(base_prefix.as_str()) else {
            continue;
        };
        // Derive the id (and name) through the same helper the indexer uses, so
        // the rewritten row is the one its next `upsert_repository` replaces
        // rather than a duplicate.
        let new_root = format!("{wt_prefix}{suffix}/");
        let retargeted = crate::indexer::package::git::RepositoryInfo::from_root_path(
            Utf8PathBuf::from(&new_root),
            None,
        );

        // Update the child before the parent so the FK never dangles.
        tx.execute(
            "UPDATE packages SET repository_id = ?1 WHERE repository_id = ?2",
            rusqlite::params![retargeted.id, old_id],
        )?;
        tx.execute(
            "UPDATE repositories SET id = ?1, name = ?2, root_path = ?3 WHERE id = ?4",
            rusqlite::params![retargeted.id, retargeted.name, new_root, old_id],
        )?;
    }

    // Absolute manifest paths are the only other rooted values. An exact prefix
    // comparison rather than LIKE, whose `_` and `%` would be wildcards in a
    // path.
    tx.execute(
        "UPDATE packages
         SET manifest_path = ?1 || substr(manifest_path, length(?2) + 1)
         WHERE substr(manifest_path, 1, length(?2)) = ?2",
        rusqlite::params![wt_prefix, base_prefix],
    )?;

    backfill_content_hashes(&tx, plan.base_repo_path.as_path())
        .context("Failed to backfill content hashes from the base working tree")?;

    for (key, value) in [
        ("seeded_from_repo_id", plan.base_repo_id.as_str()),
        ("seeded_from_path", plan.base_repo_path.as_str()),
    ] {
        tx.execute(
            "INSERT INTO index_metadata(key, value, updated_at) VALUES (?1, ?2, unixepoch())
             ON CONFLICT(key) DO UPDATE SET value=excluded.value, updated_at=excluded.updated_at",
            rusqlite::params![key, value],
        )?;
    }
    tx.execute(
        "INSERT INTO index_metadata(key, value, updated_at)
         VALUES ('seeded_at', CAST(unixepoch() AS TEXT), unixepoch())
         ON CONFLICT(key) DO UPDATE SET value=excluded.value, updated_at=excluded.updated_at",
        [],
    )?;

    tx.commit().context("Failed to commit seed rewrite")?;
    Ok(())
}

/// Fill in `content_hash` for rows a pre-upgrade index left NULL.
///
/// Only rows whose stored mtime and size still match the base repo's file on
/// disk are trusted; anything else stays NULL and gets reparsed, which is
/// correct if slower. Guessing there, by hashing whatever the file holds now,
/// would make the seeded index claim a changed file was unchanged.
///
/// Reads the base working tree, never the base database, so it cannot contend
/// with a live watcher pass on the base.
fn backfill_content_hashes(tx: &rusqlite::Transaction<'_>, base_root: &Utf8Path) -> Result<usize> {
    let pending: Vec<(String, i64, u64)> = {
        let mut stmt = tx.prepare(
            "SELECT file_path, mtime_ns, size_bytes FROM file_fingerprints \
             WHERE content_hash IS NULL",
        )?;
        let mapped = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?.max(0) as u64,
            ))
        })?;
        mapped.collect::<rusqlite::Result<Vec<_>>>()?
    };
    if pending.is_empty() {
        return Ok(0);
    }

    let pending_count = pending.len();
    let mut filled = 0usize;
    for (rel, mtime_ns, size_bytes) in pending {
        let abs = base_root.join(&rel);
        let Some(hash) = verified_content_hash(&abs, mtime_ns, size_bytes) else {
            continue;
        };
        tx.execute(
            "UPDATE file_fingerprints SET content_hash = ?1 WHERE file_path = ?2",
            rusqlite::params![hash, rel],
        )?;
        filled += 1;
    }

    tracing::info!(
        base = %base_root,
        filled,
        pending = pending_count,
        "backfilled content hashes for a pre-upgrade base index"
    );
    Ok(filled)
}

/// Hash a file only when it provably still holds the content the fingerprint's
/// stat referred to, else `None`.
///
/// The stat must match before the read, the byte count must match after it, and
/// the mtime must not have moved across it. Nothing gates a watcher pass or an
/// editor on the base repo, so a write landing between the stat and the read
/// would otherwise store a hash describing content the fingerprint never
/// referred to, and the seeded index would call a changed file unchanged.
fn verified_content_hash(abs: &Utf8Path, mtime_ns: i64, size_bytes: u64) -> Option<String> {
    let before = std::fs::metadata(abs.as_std_path()).ok()?;
    if modified_ns(&before) != mtime_ns || before.len() != size_bytes {
        return None;
    }

    let bytes = std::fs::read(abs.as_std_path()).ok()?;
    if bytes.len() as u64 != size_bytes {
        return None;
    }

    // Re-stat rather than trust the pre-read stat: a same-size write across the
    // read would pass the length check above but move the mtime.
    let after = std::fs::metadata(abs.as_std_path()).ok()?;
    if modified_ns(&after) != mtime_ns || after.len() != size_bytes {
        return None;
    }

    Some(crate::indexer::pipeline::utils::content_hash_hex(&bytes))
}

/// A file's mtime as nanoseconds since the epoch, saturating at `i64::MAX`.
fn modified_ns(meta: &std::fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::sqlite::SqliteStore;

    /// Build a git repo with one commit at `root`.
    fn init_repo(root: &Utf8Path) -> git2::Repository {
        let repo = git2::Repository::init(root.as_std_path()).unwrap();
        std::fs::write(root.join("lib.rs").as_std_path(), "pub fn probe() {}\n").unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(std::path::Path::new("lib.rs")).unwrap();
        index.write().unwrap();
        let tree_id = index.write_tree().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        let sig = git2::Signature::now("Test", "test@example.com").unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "init", &tree, &[])
            .unwrap();
        drop(tree);
        drop(index);
        repo
    }

    fn utf8(p: &std::path::Path) -> Utf8PathBuf {
        Utf8PathBuf::from_path_buf(p.to_path_buf()).unwrap()
    }

    #[test]
    fn resolves_linked_worktree_to_its_main_repo() {
        let base_temp = tempfile::tempdir().unwrap();
        let base = utf8(base_temp.path());
        let repo = init_repo(&base);

        let wt_temp = tempfile::tempdir().unwrap();
        let wt = utf8(wt_temp.path()).join("feature");
        repo.worktree("feature", wt.as_std_path(), None).unwrap();

        let resolved = resolve_base_repo(&wt).expect("worktree must resolve to a base");
        assert_eq!(
            std::fs::canonicalize(resolved.as_std_path()).unwrap(),
            std::fs::canonicalize(base.as_std_path()).unwrap()
        );
    }

    #[test]
    fn main_repo_resolves_to_none() {
        let base_temp = tempfile::tempdir().unwrap();
        let base = utf8(base_temp.path());
        init_repo(&base);
        assert_eq!(resolve_base_repo(&base), None);
    }

    #[test]
    fn non_git_directory_resolves_to_none() {
        let temp = tempfile::tempdir().unwrap();
        let dir = utf8(temp.path());
        std::fs::write(dir.join("Cargo.toml").as_std_path(), b"[package]\n").unwrap();
        assert_eq!(resolve_base_repo(&dir), None);
    }

    #[test]
    fn missing_path_resolves_to_none() {
        assert_eq!(
            resolve_base_repo(Utf8Path::new("/nonexistent/definitely/not/here")),
            None
        );
    }

    /// A real (empty) Tantivy index, built through the project's own constructor
    /// so seed validation sees what the daemon would have written.
    fn write_tantivy_index(data_dir: &Utf8Path) {
        drop(
            crate::storage::tantivy::TantivyIndex::open_or_create(&data_dir.join(TANTIVY_DIR))
                .unwrap(),
        );
    }

    /// Stand-in for the LanceDB directory: seeding only copies it, so its
    /// contents are opaque here.
    fn write_vectors_dir(data_dir: &Utf8Path) {
        std::fs::create_dir_all(data_dir.join("vectors/symbols.lance").as_std_path()).unwrap();
        std::fs::write(
            data_dir
                .join("vectors/symbols.lance/data.bin")
                .as_std_path(),
            b"vectors",
        )
        .unwrap();
    }

    /// Build a base data dir that looks like a completed index.
    fn seeded_base_data_dir(data_dir: &Utf8Path, base_root: &Utf8Path) -> String {
        std::fs::create_dir_all(data_dir.as_std_path()).unwrap();
        let db_path = data_dir.join("code-intelligence.db");
        let store = SqliteStore::open(&db_path).unwrap();
        store.init().unwrap();
        let repo_id = {
            let conn = store.write().unwrap();
            crate::storage::sqlite::queries::files::upsert_file_fingerprint(
                &conn,
                "lib.rs",
                4242,
                17,
                Some("aaaabbbbccccddddeeeeffff00001111"),
            )
            .unwrap();
            conn.execute(
                "INSERT INTO index_runs(started_at, duration_ms, files_scanned, files_indexed, \
                 files_skipped, files_unchanged, files_deleted, symbols_indexed) \
                 VALUES (1, 2, 3, 4, 0, 0, 0, 0)",
                [],
            )
            .unwrap();
            let repo_id = sha256_hex(base_root.as_str());
            conn.execute(
                "INSERT INTO repositories(id, name, root_path, vcs_type) VALUES (?1, 'base', ?2, 'git')",
                rusqlite::params![repo_id, base_root.as_str()],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO packages(id, repository_id, name, manifest_path, package_type) \
                 VALUES ('pkg1', ?1, 'base', ?2, 'cargo')",
                rusqlite::params![repo_id, format!("{base_root}/Cargo.toml")],
            )
            .unwrap();
            repo_id
        };
        drop(store);

        write_tantivy_index(data_dir);
        write_vectors_dir(data_dir);

        repo_id
    }

    /// Independent restatement of the id derivation, so the test pins the value
    /// rather than echoing the implementation.
    fn sha256_hex(input: &str) -> String {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(input.as_bytes());
        format!("{:x}", hasher.finalize())
    }

    #[test]
    fn seeding_copies_fingerprints_clears_runs_and_rewrites_repo_rows() {
        let temp = tempfile::tempdir().unwrap();
        let root = utf8(temp.path());
        let base_root = root.join("base");
        let wt_root = root.join("feature");
        std::fs::create_dir_all(base_root.as_std_path()).unwrap();
        std::fs::create_dir_all(wt_root.as_std_path()).unwrap();

        let base_data = root.join("data/base");
        let wt_data = root.join("data/feature");
        let base_repo_id = seeded_base_data_dir(&base_data, &base_root);
        std::fs::create_dir_all(wt_data.as_std_path()).unwrap();

        let plan = SeedPlan {
            base_repo_path: base_root.clone(),
            base_repo_id: base_repo_id.clone(),
            base_data_dir: base_data.clone(),
            worktree_path: wt_root.clone(),
            worktree_data_dir: wt_data.clone(),
        };
        seed_index_from_base(&plan).unwrap();

        // Tantivy and LanceDB artifacts came across.
        assert!(wt_data
            .join("tantivy-index/meta.json")
            .as_std_path()
            .is_file());
        assert!(wt_data
            .join("vectors/symbols.lance/data.bin")
            .as_std_path()
            .is_file());

        let store = SqliteStore::open(&wt_data.join("code-intelligence.db")).unwrap();
        store.init().unwrap();
        let conn = store.write().unwrap();

        // Fingerprints survive, hash included. This is the delta plan.
        let row = crate::storage::sqlite::queries::files::get_file_fingerprint(&conn, "lib.rs")
            .unwrap()
            .unwrap();
        assert_eq!(row.mtime_ns, 4242);
        assert_eq!(
            row.content_hash.as_deref(),
            Some("aaaabbbbccccddddeeeeffff00001111")
        );

        // index_runs is empty, which is what forces one InitialBind pass.
        let runs: i64 = conn
            .query_row("SELECT COUNT(*) FROM index_runs", [], |r| r.get(0))
            .unwrap();
        assert_eq!(runs, 0);

        // The repositories row now points at the worktree, with the id and name
        // the next index pass will derive for that root.
        let (new_id, new_name, new_root): (String, String, String) = conn
            .query_row("SELECT id, name, root_path FROM repositories", [], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })
            .unwrap();
        let expected_root = format!("{wt_root}/");
        assert_eq!(new_root, expected_root);
        assert_eq!(new_name, "feature");
        assert_eq!(
            new_id,
            sha256_hex(&expected_root),
            "the rewritten id must be the one the indexer computes for the worktree root"
        );
        assert_ne!(new_id, base_repo_id);

        // The package FK followed the id rewrite and its manifest path moved.
        let (pkg_repo, manifest): (String, String) = conn
            .query_row(
                "SELECT repository_id, manifest_path FROM packages",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(pkg_repo, new_id, "package must not be orphaned");
        assert_eq!(manifest, format!("{wt_root}/Cargo.toml"));

        // No package may reference a repository row that is not there.
        let orphans: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM packages p \
                 LEFT JOIN repositories r ON r.id = p.repository_id WHERE r.id IS NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(orphans, 0, "no package may be left orphaned");

        // Provenance is recorded for the dashboard and for debugging.
        let seeded_from: String = conn
            .query_row(
                "SELECT value FROM index_metadata WHERE key = 'seeded_from_path'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(seeded_from, base_root.as_str());
    }

    #[test]
    fn seeding_backfills_content_hashes_missing_from_a_legacy_base() {
        let temp = tempfile::tempdir().unwrap();
        let root = utf8(temp.path());
        let base_root = root.join("base");
        let wt_root = root.join("feature");
        std::fs::create_dir_all(base_root.as_std_path()).unwrap();
        std::fs::create_dir_all(wt_root.as_std_path()).unwrap();

        // A real file in the base tree, and the same bytes in the worktree.
        let body = "pub fn probe() -> usize { 1 }\n";
        std::fs::write(base_root.join("lib.rs").as_std_path(), body).unwrap();
        std::fs::write(wt_root.join("lib.rs").as_std_path(), body).unwrap();
        let meta = std::fs::metadata(base_root.join("lib.rs").as_std_path()).unwrap();
        let mtime_ns = meta
            .modified()
            .unwrap()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos() as i64;

        let base_data = root.join("data/base");
        let wt_data = root.join("data/feature");
        std::fs::create_dir_all(base_data.as_std_path()).unwrap();
        {
            let store = SqliteStore::open(&base_data.join("code-intelligence.db")).unwrap();
            store.init().unwrap();
            let conn = store.write().unwrap();
            // Legacy row: correct stat, NULL hash.
            crate::storage::sqlite::queries::files::upsert_file_fingerprint(
                &conn,
                "lib.rs",
                mtime_ns,
                meta.len(),
                None,
            )
            .unwrap();
            // A row whose stat no longer matches the base file: must stay NULL,
            // because we cannot know what content it was indexed from.
            crate::storage::sqlite::queries::files::upsert_file_fingerprint(
                &conn, "stale.rs", 1, 999, None,
            )
            .unwrap();
        }
        // A populated database is only seedable alongside its search indexes.
        write_tantivy_index(&base_data);
        write_vectors_dir(&base_data);
        std::fs::create_dir_all(wt_data.as_std_path()).unwrap();

        let plan = SeedPlan {
            base_repo_path: base_root.clone(),
            base_repo_id: "base".to_string(),
            base_data_dir: base_data,
            worktree_path: wt_root,
            worktree_data_dir: wt_data.clone(),
        };
        seed_index_from_base(&plan).unwrap();

        let store = SqliteStore::open(&wt_data.join("code-intelligence.db")).unwrap();
        store.init().unwrap();
        let conn = store.write().unwrap();

        let filled = crate::storage::sqlite::queries::files::get_file_fingerprint(&conn, "lib.rs")
            .unwrap()
            .unwrap();
        assert_eq!(
            filled.content_hash.as_deref(),
            Some(crate::indexer::pipeline::utils::content_hash_hex(body.as_bytes()).as_str()),
            "a legacy row whose stat still matches must get the real hash"
        );

        let untouched =
            crate::storage::sqlite::queries::files::get_file_fingerprint(&conn, "stale.rs")
                .unwrap()
                .unwrap();
        assert_eq!(
            untouched.content_hash, None,
            "a row we cannot verify must stay NULL and be reparsed"
        );
    }

    #[test]
    fn artifact_detection_distinguishes_empty_from_populated_data_dirs() {
        let temp = tempfile::tempdir().unwrap();
        let root = utf8(temp.path());
        let empty = root.join("empty");
        std::fs::create_dir_all(empty.as_std_path()).unwrap();
        assert!(!data_dir_has_index_artifacts(&empty));

        let populated = root.join("populated");
        std::fs::create_dir_all(populated.as_std_path()).unwrap();
        std::fs::write(
            populated.join("code-intelligence.db").as_std_path(),
            b"not really a db",
        )
        .unwrap();
        assert!(data_dir_has_index_artifacts(&populated));
    }

    #[test]
    fn seeding_rejects_a_torn_tantivy_clone() {
        let temp = tempfile::tempdir().unwrap();
        let root = utf8(temp.path());
        let base_root = root.join("base");
        let wt_root = root.join("feature");
        std::fs::create_dir_all(base_root.as_std_path()).unwrap();
        std::fs::create_dir_all(wt_root.as_std_path()).unwrap();
        let base_data = root.join("data/base");
        let wt_data = root.join("data/feature");
        let base_repo_id = seeded_base_data_dir(&base_data, &base_root);

        // Corrupt the base's Tantivy metadata to stand in for a torn clone.
        std::fs::write(
            base_data.join("tantivy-index/meta.json").as_std_path(),
            b"{ this is not valid tantivy metadata",
        )
        .unwrap();
        std::fs::create_dir_all(wt_data.as_std_path()).unwrap();

        let plan = SeedPlan {
            base_repo_path: base_root,
            base_repo_id,
            base_data_dir: base_data,
            worktree_path: wt_root,
            worktree_data_dir: wt_data,
        };
        let err = seed_index_from_base(&plan)
            .expect_err("an unopenable seeded index must be reported as a seed failure");
        // Pin the cause: the seed must fail *because* the search index will not
        // open, not because of some unrelated earlier step.
        let chain = format!("{err:#}");
        assert!(
            chain.contains("Seeded Tantivy index will not open"),
            "expected the Tantivy validation to be the failure, got: {chain}"
        );
    }

    /// Seed from a base whose database is populated but which only has the
    /// named search artifacts, and return the error chain the seed produced.
    fn seed_error_with_artifacts(tantivy: bool, vectors: bool) -> String {
        let temp = tempfile::tempdir().unwrap();
        let root = utf8(temp.path());
        let base_root = root.join("base");
        let wt_root = root.join("feature");
        std::fs::create_dir_all(base_root.as_std_path()).unwrap();
        std::fs::create_dir_all(wt_root.as_std_path()).unwrap();
        let base_data = root.join("data/base");
        let wt_data = root.join("data/feature");

        std::fs::create_dir_all(base_data.as_std_path()).unwrap();
        {
            let store = SqliteStore::open(&base_data.join(DB_FILE)).unwrap();
            store.init().unwrap();
            let conn = store.write().unwrap();
            crate::storage::sqlite::queries::files::upsert_file_fingerprint(
                &conn,
                "lib.rs",
                4242,
                17,
                Some("aaaabbbbccccddddeeeeffff00001111"),
            )
            .unwrap();
        }
        if tantivy {
            write_tantivy_index(&base_data);
        }
        if vectors {
            write_vectors_dir(&base_data);
        }
        std::fs::create_dir_all(wt_data.as_std_path()).unwrap();

        let plan = SeedPlan {
            base_repo_path: base_root,
            base_repo_id: "base".to_string(),
            base_data_dir: base_data,
            worktree_path: wt_root,
            worktree_data_dir: wt_data,
        };
        let err = seed_index_from_base(&plan)
            .expect_err("a base whose search index is missing must not seed");
        format!("{err:#}")
    }

    #[test]
    fn seeding_rejects_a_base_whose_search_index_did_not_come_across() {
        // Seeded fingerprints claim every file is already indexed, and nothing
        // downstream repairs a missing search index: `open_or_create` treats an
        // absent directory as a fresh index, so `was_recreated` is false and the
        // fingerprints are never cleared. Every file would then be skipped as
        // unchanged, leaving SQLite populated and search permanently empty.
        let both_missing = seed_error_with_artifacts(false, false);
        assert!(
            both_missing.contains(TANTIVY_DIR),
            "expected the missing Tantivy index to be named, got: {both_missing}"
        );

        let tantivy_missing = seed_error_with_artifacts(false, true);
        assert!(
            tantivy_missing.contains(TANTIVY_DIR),
            "expected the missing Tantivy index to be named, got: {tantivy_missing}"
        );

        let vectors_missing = seed_error_with_artifacts(true, false);
        assert!(
            vectors_missing.contains(VECTORS_DIR),
            "expected the missing vector store to be named, got: {vectors_missing}"
        );
    }

    #[test]
    fn verified_content_hash_only_trusts_a_file_that_still_matches_its_stat() {
        let temp = tempfile::tempdir().unwrap();
        let root = utf8(temp.path());
        let path = root.join("lib.rs");
        let body = "pub fn probe() -> usize { 1 }\n";
        std::fs::write(path.as_std_path(), body).unwrap();
        let meta = std::fs::metadata(path.as_std_path()).unwrap();
        let mtime_ns = modified_ns(&meta);
        let size = meta.len();

        assert_eq!(
            verified_content_hash(&path, mtime_ns, size).as_deref(),
            Some(crate::indexer::pipeline::utils::content_hash_hex(body.as_bytes()).as_str()),
            "a quiescent file matching its stat must be hashed"
        );
        assert_eq!(
            verified_content_hash(&path, mtime_ns + 1, size),
            None,
            "a moved mtime must not be trusted"
        );
        assert_eq!(
            verified_content_hash(&path, mtime_ns, size + 1),
            None,
            "a changed size must not be trusted"
        );
        assert_eq!(
            verified_content_hash(&root.join("absent.rs"), mtime_ns, size),
            None,
            "a file that is not there must not be trusted"
        );
    }

    #[test]
    fn copy_tree_recursive_copies_nested_files() {
        let temp = tempfile::tempdir().unwrap();
        let root = utf8(temp.path());
        let src = root.join("src");
        std::fs::create_dir_all(src.join("a/b").as_std_path()).unwrap();
        std::fs::write(src.join("top.bin").as_std_path(), b"top").unwrap();
        std::fs::write(src.join("a/mid.bin").as_std_path(), b"mid").unwrap();
        std::fs::write(src.join("a/b/deep.bin").as_std_path(), b"deep").unwrap();

        let dst = root.join("dst");
        copy_tree_recursive(src.as_std_path(), dst.as_std_path()).unwrap();

        assert_eq!(
            std::fs::read(dst.join("top.bin").as_std_path()).unwrap(),
            b"top"
        );
        assert_eq!(
            std::fs::read(dst.join("a/mid.bin").as_std_path()).unwrap(),
            b"mid"
        );
        assert_eq!(
            std::fs::read(dst.join("a/b/deep.bin").as_std_path()).unwrap(),
            b"deep"
        );
    }

    #[test]
    fn clone_tree_falls_back_to_a_recursive_copy_when_clonefile_fails() {
        let temp = tempfile::tempdir().unwrap();
        let root = utf8(temp.path());
        let src = root.join("src");
        std::fs::create_dir_all(src.join("nested").as_std_path()).unwrap();
        std::fs::write(src.join("nested/a.bin").as_std_path(), b"payload").unwrap();

        // clonefile(2) requires that the destination not already exist, so an
        // existing directory forces the fallback deterministically.
        let dst = root.join("dst");
        std::fs::create_dir_all(dst.as_std_path()).unwrap();
        std::fs::write(dst.join("sentinel").as_std_path(), b"kept").unwrap();

        clone_tree(&src, &dst).unwrap();

        assert_eq!(
            std::fs::read(dst.join("nested/a.bin").as_std_path()).unwrap(),
            b"payload"
        );
        // A successful clonefile could not have left the sentinel in place, so
        // its survival is proof the recursive copy is what ran.
        assert!(
            dst.join("sentinel").as_std_path().is_file(),
            "the recursive fallback must be the path that ran"
        );
    }

    #[test]
    fn seeding_refuses_to_overwrite_an_existing_database() {
        let temp = tempfile::tempdir().unwrap();
        let root = utf8(temp.path());
        let base_root = root.join("base");
        let wt_root = root.join("feature");
        std::fs::create_dir_all(base_root.as_std_path()).unwrap();
        std::fs::create_dir_all(wt_root.as_std_path()).unwrap();
        let base_data = root.join("data/base");
        let wt_data = root.join("data/feature");
        let base_repo_id = seeded_base_data_dir(&base_data, &base_root);
        std::fs::create_dir_all(wt_data.as_std_path()).unwrap();
        std::fs::write(
            wt_data.join("code-intelligence.db").as_std_path(),
            b"pre-existing",
        )
        .unwrap();

        let plan = SeedPlan {
            base_repo_path: base_root,
            base_repo_id,
            base_data_dir: base_data,
            worktree_path: wt_root,
            worktree_data_dir: wt_data,
        };
        assert!(
            seed_index_from_base(&plan).is_err(),
            "seeding must not clobber an existing index"
        );
    }
}
