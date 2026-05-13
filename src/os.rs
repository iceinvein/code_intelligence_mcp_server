//! OS-level startup helpers.
//!
//! Currently this is just FD-limit handling. The indexer fans out parallel
//! parse work, holds onto Tantivy/LanceDB mmap'd segments, and keeps the
//! embedding/reranker GGUF files open; the macOS default soft `RLIMIT_NOFILE`
//! of 256 can be hit on large repos under indexing pressure.

use std::io;

/// Raise the soft file-descriptor limit toward the hard limit (Unix only).
///
/// Returns the (old_soft, new_soft, hard) tuple on success. No-op on
/// non-Unix platforms.
#[cfg(unix)]
pub fn raise_fd_limit_to_hard() -> io::Result<(u64, u64, u64)> {
    // SAFETY: getrlimit/setrlimit accept a zero-initialized rlimit value.
    let mut current = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    let rc = unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut current) };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }

    let old_soft = u64::from(current.rlim_cur);
    let hard = u64::from(current.rlim_max);
    if old_soft >= hard {
        return Ok((old_soft, old_soft, hard));
    }

    // On macOS, RLIMIT_NOFILE has a kernel-imposed ceiling around OPEN_MAX
    // (typically 10_240) even when rlim_max reports `infinity`. Clamp to
    // a reasonable target so setrlimit doesn't fail with EINVAL.
    #[cfg(target_os = "macos")]
    let target: u64 = hard.min(10_240);
    #[cfg(not(target_os = "macos"))]
    let target: u64 = hard;

    let new = libc::rlimit {
        rlim_cur: target,
        rlim_max: current.rlim_max,
    };
    let rc = unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &new) };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }

    Ok((old_soft, target, hard))
}

#[cfg(not(unix))]
pub fn raise_fd_limit_to_hard() -> io::Result<(u64, u64, u64)> {
    Ok((0, 0, 0))
}
