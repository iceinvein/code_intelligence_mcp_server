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
    if [[ "$OSTYPE" == "darwin"* ]]; then
        cargo build --release --features model-download,metal
    else
        cargo build --release --features model-download
    fi
fi

# Environment Configuration
export BASE_DIR="$REPO_ROOT"
export EMBEDDINGS_BACKEND="candle"
export EMBEDDINGS_AUTO_DOWNLOAD="true"

# Detect OS for Metal support (macOS)
if [[ "$OSTYPE" == "darwin"* ]]; then
    export EMBEDDINGS_DEVICE="metal"
else
    export EMBEDDINGS_DEVICE="cpu"
fi

# Optional: Set persistent storage paths to a hidden dir in the repo
export DB_PATH="$REPO_ROOT/.cimcp/code-intelligence.db"
export VECTOR_DB_PATH="$REPO_ROOT/.cimcp/vectors"
export TANTIVY_INDEX_PATH="$REPO_ROOT/.cimcp/tantivy-index"

# Run the server
exec "$BINARY"
