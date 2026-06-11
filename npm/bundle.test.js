const assert = require("node:assert");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const test = require("node:test");
const { validateBundle } = require("./bundle");

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
