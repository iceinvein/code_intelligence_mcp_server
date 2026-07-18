# Testing Guide

The server is a single Streamable HTTP daemon. The old embedded-stdio and
leader-election test workflow is no longer supported.

## Required deterministic gates

The repository pins `protoc` through `.cargo/config.toml`. The wrapper downloads
and verifies protobuf 29.3 into a locked user cache, so a system `protoc` is not
required. A production build also requires the Xcode command-line tools and
CMake:

```bash
xcode-select --install            # once, if not already installed
brew install cmake
```

Run the Rust suite with both the runtime hash backend and the native feature
disabled. This path does not compile llama.cpp, require CMake, or download a
model:

```bash
EMBEDDINGS_BACKEND=hash cargo test --no-default-features
cargo fmt --all -- --check
EMBEDDINGS_BACKEND=hash \
  cargo clippy --all-targets --no-default-features -- -D warnings
```

`EMBEDDINGS_BACKEND=hash` is a runtime choice; `--no-default-features` is the
compile-time boundary that omits llama.cpp. Before producing a release, also
compile the default Metal-enabled path:

```bash
cargo clippy --all-targets --all-features -- -D warnings
cargo build --release
```

If CMake or the pinned protobuf compiler is unavailable, package build setup
fails with an actionable install or override command. Set `CMAKE` or `PROTOC`
to explicit binaries when using a nonstandard toolchain.

The engine-only quality gate builds a temporary polyglot index and checks live
retrieval recall@5, MRR, nDCG@5, graph precision/recall, impact-set
precision/recall, canonical definitions, public exposure, and adversarial
identity behavior:

```bash
EMBEDDINGS_BACKEND=hash cargo test --no-default-features \
  --test deterministic_quality deterministic_engine_quality_gate
```

This gate is deterministic and does not invoke an answering agent, access the
network, or run the external quality benchmark.

## Benchmark harness tests

Test the Python harness itself without starting a benchmark round:

```bash
python3 -m pip install -r bench/requirements.txt
python3 -m pytest bench/tests
```

Fixture linting is also agent-free:

```bash
python3 -m bench.run validate bench/fixtures/smoke.yaml --repo-root "$PWD"
```

See `bench/README.md` before running a real external-agent round. Those rounds
are deliberately separate from the deterministic CI gates.

## UI gates

```bash
cd ui
bun install --frozen-lockfile
bun run lint
bun run test
bun run build
```

## Local daemon smoke test

Build and run the HTTP daemon in the foreground with the hash backend:

```bash
EMBEDDINGS_BACKEND=hash cargo build --no-default-features
EMBEDDINGS_BACKEND=hash ./target/debug/code-intelligence-mcp-server --port 18000
```

The public endpoints are then:

- MCP: `http://127.0.0.1:18000/mcp?repo=/absolute/path/to/repo`
- discovery: `http://127.0.0.1:18001/.well-known/mcp`
- dashboard/API: `http://127.0.0.1:18002/`
- status JSON: `http://127.0.0.1:18002/api/status`

Check the HTTP surface from another terminal:

```bash
curl --fail --silent http://127.0.0.1:18002/api/status | python3 -m json.tool
```

For a production-style lifecycle test, use the `install`, `status`, `stop`, and
`uninstall` subcommands documented in the README. MCP clients should use the
Streamable HTTP URL above; do not pipe JSON-RPC into the server process.
