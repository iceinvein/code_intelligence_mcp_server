const assert = require("node:assert");
const childProcess = require("node:child_process");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const test = require("node:test");
const { validateBundle } = require("./bundle");

function extractReleaseArchiveScript() {
	const workflow = fs.readFileSync(
		path.join(__dirname, "..", ".github", "workflows", "release.yml"),
		"utf8",
	);
	const marker = "      - name: Archive Binary Bundle\n";
	const markerIndex = workflow.indexOf(marker);
	assert.notEqual(markerIndex, -1);
	const runIndex = workflow.indexOf("        run: |\n", markerIndex);
	assert.notEqual(runIndex, -1);
	const scriptStart = runIndex + "        run: |\n".length;
	const nextStepIndex = workflow.indexOf("\n      - name:", scriptStart);
	assert.notEqual(nextStepIndex, -1);

	return workflow
		.slice(scriptStart, nextStepIndex)
		.split("\n")
		.map((line) => line.replace(/^          /, ""))
		.join("\n");
}

test("validateBundle accepts server binary, manifest, and executable producers", () => {
	const dir = fs.mkdtempSync(path.join(os.tmpdir(), "ci-bundle-"));
	try {
		fs.writeFileSync(path.join(dir, "code-intelligence-mcp-server"), "");
		fs.chmodSync(path.join(dir, "code-intelligence-mcp-server"), 0o755);
		fs.mkdirSync(path.join(dir, "producers"));
		fs.writeFileSync(
			path.join(dir, "producers", "manifest.json"),
			JSON.stringify({
				producers: [
					{
						executable: "code-intelligence-external-rust",
					},
				],
			}),
		);
		fs.writeFileSync(path.join(dir, "code-intelligence-external-rust"), "");
		fs.chmodSync(path.join(dir, "code-intelligence-external-rust"), 0o755);

		assert.deepEqual(validateBundle(dir).missing, []);
	} finally {
		fs.rmSync(dir, { recursive: true, force: true });
	}
});

test("release archive step includes root wrappers and manifest support files", () => {
	const dir = fs.mkdtempSync(path.join(os.tmpdir(), "ci-release-bundle-"));
	try {
		const releaseBinaryDir = path.join(
			dir,
			"target",
			"aarch64-apple-darwin",
			"release",
		);
		fs.mkdirSync(releaseBinaryDir, { recursive: true });
		fs.writeFileSync(
			path.join(releaseBinaryDir, "code-intelligence-mcp-server"),
			"",
		);
		fs.chmodSync(
			path.join(releaseBinaryDir, "code-intelligence-mcp-server"),
			0o755,
		);

		fs.mkdirSync(path.join(dir, "producers", "bin"), { recursive: true });
		fs.mkdirSync(path.join(dir, "producers", "lib"), { recursive: true });
		fs.mkdirSync(path.join(dir, "producers", "python"), { recursive: true });
		fs.writeFileSync(
			path.join(dir, "producers", "manifest.json"),
			JSON.stringify({
				producers: [
					{
						executable: "code-intelligence-external-python",
						support_files: [
							"producers/lib/__init__.py",
							"producers/lib/normalized.py",
							"producers/python/index.py",
						],
					},
				],
			}),
		);
		fs.writeFileSync(path.join(dir, "producers", "lib", "__init__.py"), "");
		fs.writeFileSync(path.join(dir, "producers", "lib", "normalized.py"), "");
		fs.writeFileSync(path.join(dir, "producers", "python", "index.py"), "");
		fs.writeFileSync(
			path.join(dir, "producers", "bin", "code-intelligence-external-python"),
			"",
		);
		fs.chmodSync(
			path.join(dir, "producers", "bin", "code-intelligence-external-python"),
			0o755,
		);

		childProcess.execFileSync(
			"bash",
			["-euo", "pipefail", "-c", extractReleaseArchiveScript()],
			{
				cwd: dir,
				stdio: "pipe",
			},
		);

		assert.deepEqual(validateBundle(path.join(dir, "bundle")).missing, []);
	} finally {
		fs.rmSync(dir, { recursive: true, force: true });
	}
});

test("validateBundle reports missing producer executables", () => {
	const dir = fs.mkdtempSync(path.join(os.tmpdir(), "ci-bundle-"));
	try {
		fs.writeFileSync(path.join(dir, "code-intelligence-mcp-server"), "");
		fs.chmodSync(path.join(dir, "code-intelligence-mcp-server"), 0o755);
		fs.mkdirSync(path.join(dir, "producers"));
		fs.writeFileSync(
			path.join(dir, "producers", "manifest.json"),
			JSON.stringify({
				producers: [
					{
						executable: "code-intelligence-external-rust",
					},
				],
			}),
		);

		assert.deepEqual(validateBundle(dir).missing, [
			"code-intelligence-external-rust",
		]);
	} finally {
		fs.rmSync(dir, { recursive: true, force: true });
	}
});

test("validateBundle requires manifest-declared producer support files", () => {
	const dir = fs.mkdtempSync(path.join(os.tmpdir(), "ci-bundle-"));
	try {
		fs.writeFileSync(path.join(dir, "code-intelligence-mcp-server"), "");
		fs.chmodSync(path.join(dir, "code-intelligence-mcp-server"), 0o755);
		fs.mkdirSync(path.join(dir, "producers", "bin"), { recursive: true });
		fs.writeFileSync(
			path.join(dir, "producers", "manifest.json"),
			JSON.stringify({
				producers: [
					{
						executable: "producers/bin/code-intelligence-external-python",
						support_files: [
							"producers/lib/__init__.py",
							"producers/lib/normalized.py",
							"producers/python/index.py",
						],
					},
				],
			}),
		);
		fs.writeFileSync(
			path.join(dir, "producers", "bin", "code-intelligence-external-python"),
			"",
		);
		fs.chmodSync(
			path.join(dir, "producers", "bin", "code-intelligence-external-python"),
			0o755,
		);

		assert.deepEqual(validateBundle(dir).missing, [
			"producers/lib/__init__.py",
			"producers/lib/normalized.py",
			"producers/python/index.py",
		]);
	} finally {
		fs.rmSync(dir, { recursive: true, force: true });
	}
});

test("validateBundle reports malformed manifests", () => {
	const dir = fs.mkdtempSync(path.join(os.tmpdir(), "ci-bundle-"));
	try {
		fs.writeFileSync(path.join(dir, "code-intelligence-mcp-server"), "");
		fs.chmodSync(path.join(dir, "code-intelligence-mcp-server"), 0o755);
		fs.mkdirSync(path.join(dir, "producers"));
		fs.writeFileSync(path.join(dir, "producers", "manifest.json"), "{}");

		assert.deepEqual(validateBundle(dir).missing, ["producers/manifest.json"]);
	} finally {
		fs.rmSync(dir, { recursive: true, force: true });
	}
});
