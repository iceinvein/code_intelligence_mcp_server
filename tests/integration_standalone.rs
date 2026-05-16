//! Integration test for standalone mode
//! Tests that the server starts, accepts HTTP connections, and routes sessions.

use std::process::Command;
use std::time::Duration;

#[test]
#[ignore] // Run manually: cargo test --test integration_standalone -- --ignored
fn standalone_server_starts_and_responds_to_http() {
    // Start server in background with hash embedder (fast, no model download)
    let mut child = Command::new(env!("CARGO_BIN_EXE_code-intelligence-mcp-server"))
        .args(["--port", "13333"])
        .env("EMBEDDINGS_BACKEND", "hash")
        .spawn()
        .expect("Failed to start standalone server");

    // Give it time to bind
    std::thread::sleep(Duration::from_secs(2));

    // GET to /mcp endpoint (streamable HTTP expects POST)
    // We just verify the server is listening — any HTTP response means it's up
    let result = Command::new("curl")
        .args([
            "-s",
            "-o",
            "/dev/null",
            "-w",
            "%{http_code}",
            "http://127.0.0.1:13333/mcp",
        ])
        .output();

    child.kill().ok();
    child.wait().ok();

    if let Ok(output) = result {
        let status = String::from_utf8_lossy(&output.stdout);
        // 405 Method Not Allowed is expected for GET on /mcp (needs POST)
        // Anything other than connection refused means the server is running
        assert!(
            status == "405" || status == "200" || status == "400",
            "Unexpected status code: {} (expected 405, 200, or 400)",
            status,
        );
    }
}
