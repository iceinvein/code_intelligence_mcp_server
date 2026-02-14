#!/bin/bash

# Resolve the repository root (assuming script is in scripts/)
SCRIPT_DIR="$( cd "$( dirname "${BASH_SOURCE[0]}" )" && pwd )"
REPO_ROOT="$(dirname "$SCRIPT_DIR")"

# Binary path
BINARY="$REPO_ROOT/target/release/code-intelligence-mcp-server"

# Check if binary exists, if not build it
if [ ! -f "$BINARY" ]; then
    echo "Binary not found, building..." >&2
    cd "$REPO_ROOT"
    cargo build --release
fi

# Environment Configuration
export BASE_DIR="$REPO_ROOT"
# Default backend is JinaCode (768-dim) -- better for code search
# Override with EMBEDDINGS_BACKEND=fastembed for BGE (384-dim) if needed
export EMBEDDINGS_AUTO_DOWNLOAD="true"

# Metal GPU acceleration (macOS)
export EMBEDDINGS_DEVICE="metal"

# Per-repo data auto-derived from BASE_DIR under ~/.code-intelligence/repos/<hash>/

# Run the server
exec "$BINARY"
