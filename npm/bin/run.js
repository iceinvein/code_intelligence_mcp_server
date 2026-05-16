#!/usr/bin/env node
//
// Thin shim that locates the `code-intelligence-mcp-server` binary and
// execs it with the user's arguments. v4 introduced Homebrew as the
// primary distribution path; this wrapper exists so the v3-era
// `npx -y @iceinvein/code-intelligence-mcp ...` muscle memory keeps
// working.
//
// Resolution order:
//   1. CODE_INTELLIGENCE_MCP_BINARY env var (escape hatch for dev).
//   2. `code-intelligence-mcp-server` on PATH (typical Homebrew
//      install at /opt/homebrew/bin/).
//   3. Bundled binary at npm/bin/code-intelligence-mcp-server, which
//      the package's postinstall script downloaded from GitHub
//      Releases.
//
// We deliberately do not inject the v3 environment defaults
// (BASE_DIR, EMBEDDINGS_BACKEND=jinacode, EMBEDDINGS_MODEL_REPO,
// EMBEDDINGS_MAX_THREADS, METRICS_ENABLED). The v4 daemon ignores
// those, and overriding them would mask real configuration issues.

"use strict";

const { spawn, spawnSync } = require("node:child_process");
const path = require("node:path");
const fs = require("node:fs");

const BINARY_NAME = "code-intelligence-mcp-server";
const BUNDLED_BINARY = path.join(__dirname, BINARY_NAME);

function exists(filePath) {
	try {
		fs.accessSync(filePath, fs.constants.X_OK);
		return true;
	} catch {
		return false;
	}
}

function findOnPath() {
	const result = spawnSync("which", [BINARY_NAME], { encoding: "utf8" });
	if (result.status !== 0) return null;
	const candidate = result.stdout.trim();
	if (!candidate) return null;
	// `which` returns the bundled binary too if `npm/bin/` happens to be
	// on PATH (rare but possible with `npm link`). Skip that case so we
	// don't loop back into ourselves.
	if (candidate === BUNDLED_BINARY) return null;
	return candidate;
}

function resolveBinary() {
	const override = process.env.CODE_INTELLIGENCE_MCP_BINARY;
	if (override) {
		if (!exists(override)) {
			console.error(
				`[code-intelligence-mcp] CODE_INTELLIGENCE_MCP_BINARY=${override} is not executable.`,
			);
			process.exit(1);
		}
		return override;
	}

	const onPath = findOnPath();
	if (onPath) return onPath;

	if (exists(BUNDLED_BINARY)) return BUNDLED_BINARY;

	console.error(
		`[code-intelligence-mcp] Binary not found.

Tried:
  - CODE_INTELLIGENCE_MCP_BINARY env var (not set)
  - ${BINARY_NAME} on PATH
  - ${BUNDLED_BINARY}

Install via Homebrew (recommended):
  brew tap iceinvein/tap
  brew install code-intelligence-mcp

Or reinstall the npm package to re-trigger the postinstall download:
  npm install -g @iceinvein/code-intelligence-mcp
`,
	);
	process.exit(1);
}

const binary = resolveBinary();
const child = spawn(binary, process.argv.slice(2), {
	env: process.env,
	stdio: "inherit",
});

child.on("exit", (code, signal) => {
	if (signal) {
		process.kill(process.pid, signal);
	} else {
		process.exit(code ?? 1);
	}
});

// Forward common termination signals so the underlying binary exits
// cleanly when the wrapper's process is killed.
for (const sig of ["SIGINT", "SIGTERM", "SIGQUIT", "SIGHUP"]) {
	process.on(sig, () => child.kill(sig));
}
