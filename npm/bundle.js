"use strict";

const fs = require("node:fs");
const path = require("node:path");

const SERVER_BINARY = "code-intelligence-mcp-server";

function isExecutable(filePath) {
	try {
		fs.accessSync(filePath, fs.constants.X_OK);
		return true;
	} catch {
		return false;
	}
}

function readManifest(binDir) {
	const manifestPath = path.join(binDir, "producers", "manifest.json");
	return JSON.parse(fs.readFileSync(manifestPath, "utf8"));
}

function validateBundle(binDir) {
	const missing = [];

	if (!isExecutable(path.join(binDir, SERVER_BINARY))) {
		missing.push(SERVER_BINARY);
	}

	const manifest = readManifest(binDir);
	for (const producer of manifest.producers || []) {
		if (!isExecutable(path.join(binDir, producer.executable))) {
			missing.push(producer.executable);
		}
	}

	return { missing };
}

module.exports = {
	validateBundle,
};
