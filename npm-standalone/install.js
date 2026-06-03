const fs = require("fs");
const path = require("path");
const axios = require("axios");
const tar = require("tar");
const os = require("os");
const crypto = require("crypto");

const REPO = "iceinvein/code_intelligence_mcp_server";
const BINARY_NAME = "code-intelligence-mcp-server";
// We use the version from package.json to fetch the matching tag
const VERSION = "v" + require("./package.json").version;

const MAPPING = {
	darwin: {
		arm64: "aarch64-apple-darwin",
	},
};

async function install() {
	const platform = os.platform();
	const arch = os.arch();

	if (!MAPPING[platform] || !MAPPING[platform][arch]) {
		console.error(
			`\n  Code Intelligence MCP Server currently only supports macOS (Apple Silicon).\n`,
		);
		console.error(`  Detected: ${platform} ${arch}`);
		console.error(`  Supported: darwin arm64 (macOS with Apple Silicon)\n`);
		console.error(`  For updates on additional platform support, see:`);
		console.error(
			`  https://github.com/iceinvein/code_intelligence_mcp_server\n`,
		);
		process.exit(1);
	}

	const target = MAPPING[platform][arch];
	const tarFilename = `${BINARY_NAME}-${target}.tar.gz`;
	const url = `https://github.com/${REPO}/releases/download/${VERSION}/${tarFilename}`;
	const checksumUrl = `${url}.sha256`;

	const binDir = path.join(__dirname, "bin");
	const destBinary = path.join(binDir, BINARY_NAME);

	// Ensure bin dir exists
	if (!fs.existsSync(binDir)) {
		fs.mkdirSync(binDir, { recursive: true });
	}

	console.log(`Downloading ${BINARY_NAME} ${VERSION} for ${target}...`);
	console.log(`URL: ${url}`);

	try {
		const checksumResponse = await axios({
			method: "get",
			url: checksumUrl,
			responseType: "text",
		});
		const expectedSha = String(checksumResponse.data).trim().split(/\s+/)[0];
		if (!/^[a-fA-F0-9]{64}$/.test(expectedSha)) {
			throw new Error(`Invalid checksum response from ${checksumUrl}`);
		}

		const tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), "code-intel-install-"));
		const tmpTar = path.join(tmpDir, tarFilename);
		try {
			const response = await axios({
				method: "get",
				url: url,
				responseType: "stream",
			});
			const writer = fs.createWriteStream(tmpTar, { mode: 0o600 });
			response.data.pipe(writer);
			await new Promise((resolve, reject) => {
				writer.on("finish", resolve);
				writer.on("error", reject);
				response.data.on("error", reject);
			});

			const actualSha = crypto
				.createHash("sha256")
				.update(fs.readFileSync(tmpTar))
				.digest("hex");
			if (actualSha.toLowerCase() !== expectedSha.toLowerCase()) {
				throw new Error(
					`Checksum mismatch for ${tarFilename}: expected ${expectedSha}, got ${actualSha}`,
				);
			}

			await tar.x({
				C: binDir,
				file: tmpTar,
			});
		} finally {
			fs.rmSync(tmpDir, { recursive: true, force: true });
		}

		// Verify the binary exists
		if (fs.existsSync(destBinary)) {
			fs.chmodSync(destBinary, 0o755);
			console.log(`Successfully installed to ${destBinary}`);
		} else {
			console.error("Extraction failed: Binary not found after unpacking.");
			console.error(`Expected location: ${destBinary}`);
			// List contents of binDir to help debug
			console.log("Contents of bin directory:", fs.readdirSync(binDir));
			process.exit(1);
		}
	} catch (error) {
		console.error("Failed to download or install binary:", error.message);
		if (error.response && error.response.status === 404) {
			console.error(
				`Release not found. Please ensure version ${VERSION} is published on GitHub.`,
			);
		}
		process.exit(1);
	}
}

install();
