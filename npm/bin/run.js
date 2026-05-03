#!/usr/bin/env node

const { spawn } = require("node:child_process");
const path = require("node:path");
const os = require("node:os");
const fs = require("node:fs");

const BINARY_NAME = "code-intelligence-mcp-server";
const BINARY_PATH = path.join(__dirname, BINARY_NAME);

if (!fs.existsSync(BINARY_PATH)) {
	console.error(`Binary not found at ${BINARY_PATH}`);
	console.error(
		"Please try reinstalling the package: npm install -g code-intelligence-mcp",
	);
	process.exit(1);
}

// Setup Environment
const env = { ...process.env };

// 1. Default BASE_DIR to current working directory if not set
if (!env.BASE_DIR) {
	env.BASE_DIR = process.cwd();
}

// 2. Default to JinaCode backend (matches Rust defaults)
if (!env.EMBEDDINGS_BACKEND) {
	env.EMBEDDINGS_BACKEND = "jinacode";
}

// 3. Enable Auto Download
if (!env.EMBEDDINGS_AUTO_DOWNLOAD) {
	env.EMBEDDINGS_AUTO_DOWNLOAD = "true";
}

// 4. Set Better Default Model (Jina V2 Base Code)
if (!env.EMBEDDINGS_MODEL_REPO) {
	env.EMBEDDINGS_MODEL_REPO = "jinaai/jina-embeddings-v2-base-code";
}

// 5. Metal GPU Acceleration
if (!env.EMBEDDINGS_DEVICE) {
	env.EMBEDDINGS_DEVICE = "metal";
}

// 6. Limit CPU threads for embedding model (helps reduce CPU usage)
// For example, set to 50% of available cores: EMBEDDINGS_MAX_THREADS=4
// Default is 0 (auto, use all available CPUs)
if (!env.EMBEDDINGS_MAX_THREADS) {
	// Set a sensible default based on CPU count to avoid 100% CPU usage
	const cpuCount = os.cpus().length;
	// Use 50% of available CPUs, minimum 2, maximum 8
	const defaultThreads = Math.max(2, Math.min(8, Math.floor(cpuCount * 0.5)));
	env.EMBEDDINGS_MAX_THREADS = defaultThreads.toString();
	console.error(
		`[code-intelligence-mcp] Setting EMBEDDINGS_MAX_THREADS=${defaultThreads} (${cpuCount} CPUs detected)`,
	);
	console.error(
		"[code-intelligence-mcp] Set EMBEDDINGS_MAX_THREADS=0 to use all CPUs or customize as needed",
	);
}

// Disable HTTP metrics server for MCP (overkill - logs provide enough debugging info)
if (!env.METRICS_ENABLED) {
	env.METRICS_ENABLED = "false";
}

// Per-repo data paths are auto-derived by the Rust binary from BASE_DIR.
// Indexes are stored under ~/.code-intelligence/repos/<hash>/.
// Models are shared across projects under ~/.code-intelligence/models/.
// No need to set DB_PATH, VECTOR_DB_PATH, or TANTIVY_INDEX_PATH here.

// Spawn the process
const child = spawn(BINARY_PATH, process.argv.slice(2), {
	env: env,
	stdio: "inherit", // Pipe stdin/out/err directly
});

child.on("exit", (code) => {
	process.exit(code);
});

// Forward signals
process.on("SIGINT", () => child.kill("SIGINT"));
process.on("SIGTERM", () => child.kill("SIGTERM"));
