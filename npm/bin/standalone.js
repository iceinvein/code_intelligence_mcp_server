#!/usr/bin/env node

const { spawn } = require('node:child_process');
const path = require('node:path');
const os = require('node:os');
const fs = require('node:fs');

const BINARY_NAME = `code-intelligence-mcp-server${os.platform() === 'win32' ? '.exe' : ''}`;
const BINARY_PATH = path.join(__dirname, BINARY_NAME);

if (!fs.existsSync(BINARY_PATH)) {
    console.error(`Binary not found at ${BINARY_PATH}`);
    console.error('Please try reinstalling the package: npm install -g @iceinvein/code-intelligence-mcp');
    process.exit(1);
}

// Pass through all args, prepend --standalone
const args = ['--standalone', ...process.argv.slice(2)];

const child = spawn(BINARY_PATH, args, {
    stdio: 'inherit'
});

child.on('exit', (code) => process.exit(code));
process.on('SIGINT', () => child.kill('SIGINT'));
process.on('SIGTERM', () => child.kill('SIGTERM'));
