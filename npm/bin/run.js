#!/usr/bin/env node

const { spawn } = require('child_process');
const path = require('path');
const os = require('os');
const fs = require('fs');

const BINARY_NAME = 'code-intelligence-mcp-server' + (os.platform() === 'win32' ? '.exe' : '');
const BINARY_PATH = path.join(__dirname, BINARY_NAME);

if (!fs.existsSync(BINARY_PATH)) {
    console.error(`Binary not found at ${BINARY_PATH}`);
    console.error('Please try reinstalling the package: npm install -g code-intelligence-mcp');
    process.exit(1);
}

// Setup Environment
const env = { ...process.env };

// 1. Default BASE_DIR to current working directory if not set
if (!env.BASE_DIR) {
    env.BASE_DIR = process.cwd();
}

// 2. Default to Candle backend (local AI)
if (!env.EMBEDDINGS_BACKEND) {
    env.EMBEDDINGS_BACKEND = 'fastembed';
}

// 3. Enable Auto Download
if (!env.EMBEDDINGS_AUTO_DOWNLOAD) {
    env.EMBEDDINGS_AUTO_DOWNLOAD = 'true';
}

// 4. Set Better Default Model (Jina V2 Base Code)
if (!env.EMBEDDINGS_MODEL_REPO) {
    env.EMBEDDINGS_MODEL_REPO = 'BAAI/bge-base-en-v1.5';
}

// 5. Metal Acceleration for macOS
if (os.platform() === 'darwin' && !env.EMBEDDINGS_DEVICE) {
    env.EMBEDDINGS_DEVICE = 'metal';
} else if (!env.EMBEDDINGS_DEVICE) {
    env.EMBEDDINGS_DEVICE = 'cpu';
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
    console.error(`[code-intelligence-mcp] Setting EMBEDDINGS_MAX_THREADS=${defaultThreads} (${cpuCount} CPUs detected)`);
    console.error('[code-intelligence-mcp] Set EMBEDDINGS_MAX_THREADS=0 to use all CPUs or customize as needed');
}

// 5. Set persistence paths to be inside the project (BASE_DIR/.cimcp) 
// if not explicitly overridden. This keeps indexes local to the project.
const cimcpDir = path.join(env.BASE_DIR, '.cimcp');

// Ensure .cimcp directory exists
if (!fs.existsSync(cimcpDir)) {
    try {
        fs.mkdirSync(cimcpDir, { recursive: true });
    } catch (e) {
        console.error(`Failed to create .cimcp directory at ${cimcpDir}:`, e.message);
        // Continue, the server might handle it or fail later
    }
}

if (!env.DB_PATH) env.DB_PATH = path.join(cimcpDir, 'code-intelligence.db');
if (!env.VECTOR_DB_PATH) env.VECTOR_DB_PATH = path.join(cimcpDir, 'vectors');
if (!env.TANTIVY_INDEX_PATH) env.TANTIVY_INDEX_PATH = path.join(cimcpDir, 'tantivy-index');

// Also set model dir - use GLOBAL cache to avoid downloading models for every project
// Models are shared across projects, but indexes remain local
if (!env.EMBEDDINGS_MODEL_DIR) {
    // Use platform-appropriate global cache location
    if (os.platform() === 'darwin') {
        // macOS: ~/Library/Application Support/cimcp/embeddings-cache
        env.EMBEDDINGS_MODEL_DIR = path.join(os.homedir(), 'Library', 'Application Support', 'cimcp', 'embeddings-cache');
    } else if (os.platform() === 'linux') {
        // Linux: ~/.local/share/cimcp/embeddings-cache
        const xdgDataHome = process.env.XDG_DATA_HOME || path.join(os.homedir(), '.local', 'share');
        env.EMBEDDINGS_MODEL_DIR = path.join(xdgDataHome, 'cimcp', 'embeddings-cache');
    } else if (os.platform() === 'win32') {
        // Windows: %APPDATA%/cimcp/embeddings-cache
        env.EMBEDDINGS_MODEL_DIR = path.join(process.env.APPDATA || path.join(os.homedir(), 'AppData', 'Roaming'), 'cimcp', 'embeddings-cache');
    } else {
        // Fallback to ~/.cimcp/embeddings-cache
        env.EMBEDDINGS_MODEL_DIR = path.join(os.homedir(), '.cimcp', 'embeddings-cache');
    }

    // Ensure global model cache directory exists
    if (!fs.existsSync(env.EMBEDDINGS_MODEL_DIR)) {
        try {
            fs.mkdirSync(env.EMBEDDINGS_MODEL_DIR, { recursive: true });
        } catch (e) {
            console.error(`Failed to create global embeddings cache at ${env.EMBEDDINGS_MODEL_DIR}:`, e.message);
            console.warn('Falling back to local project cache for this session');
            env.EMBEDDINGS_MODEL_DIR = path.join(cimcpDir, 'embeddings-model');
        }
    }
}

// Spawn the process
const child = spawn(BINARY_PATH, process.argv.slice(2), {
    env: env,
    stdio: 'inherit' // Pipe stdin/out/err directly
});

child.on('exit', (code) => {
    process.exit(code);
});

// Forward signals
process.on('SIGINT', () => child.kill('SIGINT'));
process.on('SIGTERM', () => child.kill('SIGTERM'));
