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
    env.EMBEDDINGS_BACKEND = 'candle';
}

// 3. Enable Auto Download
if (!env.EMBEDDINGS_AUTO_DOWNLOAD) {
    env.EMBEDDINGS_AUTO_DOWNLOAD = 'true';
}

// 4. Metal Acceleration for macOS
if (os.platform() === 'darwin' && !env.EMBEDDINGS_DEVICE) {
    env.EMBEDDINGS_DEVICE = 'metal';
} else if (!env.EMBEDDINGS_DEVICE) {
    env.EMBEDDINGS_DEVICE = 'cpu';
}

// 5. Set persistence paths to be inside the project (BASE_DIR/.cimcp) 
// if not explicitly overridden. This keeps indexes local to the project.
const cimcpDir = path.join(env.BASE_DIR, '.cimcp');
if (!env.DB_PATH) env.DB_PATH = path.join(cimcpDir, 'code-intelligence.db');
if (!env.VECTOR_DB_PATH) env.VECTOR_DB_PATH = path.join(cimcpDir, 'vectors');
if (!env.TANTIVY_INDEX_PATH) env.TANTIVY_INDEX_PATH = path.join(cimcpDir, 'tantivy-index');
// Also set model dir to local project cache if not set globally
if (!env.EMBEDDINGS_MODEL_DIR) env.EMBEDDINGS_MODEL_DIR = path.join(cimcpDir, 'embeddings-model');

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
