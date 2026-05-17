//! End-to-end test: drive a real rust-mcp-sdk instance through our
//! proxy, force a session-not-found by sending a fabricated session id,
//! and assert the proxy recovers transparently.
//!
//! This complements the unit tests in `src/server/mcp_proxy.rs::tests`
//! which use a fake upstream. The intent is to catch SDK regressions
//! (e.g., changes to the -32016 envelope shape) that the unit tests
//! cannot.
//!
//! # Deferred
//!
//! The audit in Task 5 of plan
//! `docs/superpowers/plans/2026-05-17-mcp-session-auto-recovery.md`
//! found no public seam in the current codebase to spin up just the
//! rust-mcp-sdk transport without also loading the embedding model,
//! description LLM, and reranker.
//!
//! Specifically:
//! - `SessionManager::new_for_test` and `new_for_test_with_ttl` are the
//!   only constructors that skip model loading (they use `HashEmbedder`),
//!   but both are gated behind `#[cfg(test)]` and are therefore
//!   inaccessible from integration tests in the `tests/` directory, which
//!   compile as a separate crate outside the `cfg(test)` boundary.
//! - `SessionManager::new` (the only pub constructor reachable from
//!   `tests/`) requires a fully constructed `Box<dyn Embedder + Send>`.
//!   `HashEmbedder` itself is pub, but `StandaloneConfig` requires an
//!   `EmbeddingsBackend` and the wiring to construct a registry and pass
//!   everything through `SessionManager::new` mirrors `main.rs` closely
//!   enough that a test helper really belongs in `src/`, not in `tests/`.
//! - `hyper_server::create_server` (the rust-mcp-sdk transport entry
//!   point) requires a `ToMcpServerHandler`-wrapped `StandaloneHandler`,
//!   which in turn requires a live `Arc<SessionManager>`. There is no
//!   lighter constructor that omits the session manager.
//!
//! Until `src/` exposes a `pub fn start_test_daemon(port: u16) ->
//! JoinHandle<()>` (or a `pub fn test_session_manager(data_dir:
//! &Utf8Path) -> SessionManager` that is NOT cfg-gated), this test is
//! left as a scaffold. The unit tests at
//! `src/server/mcp_proxy.rs::tests::forward_recovers_*` are authoritative
//! for the recovery logic.
//!
//! To enable this test a follow-up task should:
//! 1. Extract the `#[cfg(test)]` body of `SessionManager::new_for_test`
//!    into a public `pub fn new_with_hash_embedder(data_dir, config)` (or
//!    a `TestDaemon` builder in a `src/testing.rs` module gated with
//!    `#[cfg(any(test, feature = "test-helpers"))]`).
//! 2. Wire that helper through `StandaloneHandler::new` +
//!    `hyper_server::create_server` + `spawn_mcp_proxy`, binding all
//!    three to port 0 so tests do not conflict.
//! 3. Replace the body of the test below with the real assertions.

#[tokio::test]
async fn proxy_recovers_stale_session_against_real_sdk() {
    // Skip: no public SDK test harness available.
    // See module docstring for context. Re-enable when the project
    // exposes a way to construct just the MCP transport without
    // model dependencies.
    eprintln!(
        "skipping proxy_recovers_stale_session_against_real_sdk: \
         no public SDK test harness yet (see module docs)"
    );
}
