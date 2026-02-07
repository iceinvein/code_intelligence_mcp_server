const fs = require('fs');
const path = require('path');
const axios = require('axios');
const tar = require('tar');
const os = require('os');

const REPO = 'iceinvein/code_intelligence_mcp_server';
const BINARY_NAME = 'code-intelligence-mcp-server';
// We use the version from package.json to fetch the matching tag
const VERSION = 'v' + require('./package.json').version;

const MAPPING = {
    'darwin': {
        'arm64': 'aarch64-apple-darwin'
    },
    'linux': {
        'x64': 'x86_64-unknown-linux-gnu'
    }
};

async function install() {
    const platform = os.platform();
    const arch = os.arch();

    if (!MAPPING[platform] || !MAPPING[platform][arch]) {
        console.error(`Unsupported platform: ${platform} ${arch}`);
        process.exit(1);
    }

    const target = MAPPING[platform][arch];
    const extension = platform === 'win32' ? '.exe' : '';
    const tarFilename = `${BINARY_NAME}-${target}.tar.gz`;
    const url = `https://github.com/${REPO}/releases/download/${VERSION}/${tarFilename}`;
    
    const binDir = path.join(__dirname, 'bin');
    const destBinary = path.join(binDir, BINARY_NAME + extension);

    // Ensure bin dir exists
    if (!fs.existsSync(binDir)) {
        fs.mkdirSync(binDir, { recursive: true });
    }

    console.log(`Downloading ${BINARY_NAME} ${VERSION} for ${target}...`);
    console.log(`URL: ${url}`);

    try {
        const response = await axios({
            method: 'get',
            url: url,
            responseType: 'stream'
        });

        // Pipe the tar.gz stream directly into the extractor
        const extract = tar.x({
            C: binDir,
            // We want to extract the binary, but the tar structure might depend on how it was packed.
            // The CI "tar czf" command packs the binary directly at the root of the tarball.
            // So we just extract it.
        });

        response.data.pipe(extract);

        await new Promise((resolve, reject) => {
            extract.on('finish', resolve);
            extract.on('error', reject);
        });

        // Verify the binary exists
        if (fs.existsSync(destBinary)) {
            // Make executable on unix
            if (platform !== 'win32') {
                fs.chmodSync(destBinary, 0o755);
            }
            console.log(`Successfully installed to ${destBinary}`);
        } else {
            console.error('Extraction failed: Binary not found after unpacking.');
            console.error(`Expected location: ${destBinary}`);
            // List contents of binDir to help debug
            console.log('Contents of bin directory:', fs.readdirSync(binDir));
            process.exit(1);
        }

    } catch (error) {
        console.error('Failed to download or install binary:', error.message);
        if (error.response && error.response.status === 404) {
            console.error(`Release not found. Please ensure version ${VERSION} is published on GitHub.`);
        }
        process.exit(1);
    }
}

install();
