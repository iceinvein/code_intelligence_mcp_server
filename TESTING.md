# Testing Guide

This document outlines how to test the Code Intelligence MCP Server locally.

## Automated Local Testing

We provide a script to build the server, set up a temporary workspace with dummy code, and run a sequence of JSON-RPC requests against it.

### Usage

Run the following command from the project root:

```bash
./scripts/test_local.sh
```

### What it does

1.  **Builds** the project using `cargo build`.
2.  **Creates** a temporary directory `test_workspace`.
3.  **Generates** dummy `.rs` and `.ts` files in that workspace.
4.  **Generates** a `requests.jsonl` file containing MCP JSON-RPC messages:
    *   `initialize`
    *   `refresh_index` (triggers indexing of the dummy files)
    *   `get_index_stats`
    *   `search_code` (standard search)
    *   `search_code` (intent-based "who calls...")
    *   `get_definition`
5.  **Runs** the server binary with `BASE_DIR` set to the test workspace and pipes the requests to it.
6.  **Outputs** the responses to the console and `test_workspace/output.jsonl`.

## Manual Testing

To manually test with specific queries or your own codebase:

1.  **Build the server:**
    ```bash
    cargo build --release
    ```

2.  **Run with environment variables:**
    ```bash
    export BASE_DIR=/absolute/path/to/your/repo
    export EMBEDDINGS_BACKEND=hash  # Use 'fastembed' for real embeddings (requires download)
    
    ./target/release/code-intelligence-mcp-server
    ```

3.  **Interact via Stdio:**
    Type JSON-RPC messages into the terminal. Ensure each message is on a single line.

    *Example Init:*
    ```json
    {"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"manual-test","version":"1.0"}}}
    ```
    
    *Example Notification:*
    ```json
    {"jsonrpc":"2.0","method":"notifications/initialized"}
    ```

    *Example Tool Call:*
    ```json
    {"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"search_code","arguments":{"query":"search term"}}}
    ```

## Integration Tests

Run the Rust integration tests suite:

```bash
cargo test --test integration_index_search
```
