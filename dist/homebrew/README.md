# Homebrew distribution

The Homebrew tap lives in a separate repo: [`iceinvein/homebrew-tap`](https://github.com/iceinvein/homebrew-tap). The canonical formula source is `code-intelligence-mcp.rb` in this directory; the tap repo holds a copy synced at release time.

## Why two copies

Homebrew taps must be at the root of a dedicated `homebrew-*` repo, but version control still needs a canonical place inside the project tree. Keeping the formula here means:

- Formula changes are reviewed in the same PR as the code change that motivates them.
- The release workflow can rewrite the `sha256` line in-tree and copy the result to the tap repo.
- A reader of this repo can see the install instructions without context switching.

## Tap repo setup (one time)

1. Create `https://github.com/iceinvein/homebrew-tap`.
2. Copy `code-intelligence-mcp.rb` into the root of that repo.
3. Tag both repos in lockstep so users see consistent versions: `git tag v4.0.0` on this repo (triggers the binary build); after the release workflow uploads the tarball, `scripts/release.sh` rewrites the `sha256` and pushes the formula update to the tap.

After setup, users install via:

```bash
brew tap iceinvein/tap
brew install code-intelligence-mcp
brew services start code-intelligence-mcp
```

## How the release flow keeps the formula in sync

The `Release` GitHub Actions workflow:

1. Builds `code-intelligence-mcp-server` for `aarch64-apple-darwin`.
2. Tars it as `code-intelligence-mcp-server-aarch64-apple-darwin.tar.gz`.
3. Uploads the tarball to the GitHub Release for the tag.
4. Computes its `sha256` and prints it to the workflow summary.

The release script (`scripts/release.sh`) rewrites the `version` and the `sha256` line in this directory's `code-intelligence-mcp.rb` after the workflow completes, then commits and tags the bump. The same file is mirrored into `iceinvein/homebrew-tap`.

## Service ownership

The Homebrew distribution path uses Homebrew's own service management. **Do not** run the binary's `install` / `uninstall` / `start` / `stop` subcommands from a brew install — those write a separate launchd plist (`~/Library/LaunchAgents/com.iceinvein.code-intelligence.plist`) that fights brew's plist.

For brew installs, use brew commands:

```bash
brew services start    code-intelligence-mcp
brew services stop     code-intelligence-mcp
brew services restart  code-intelligence-mcp
brew services info     code-intelligence-mcp
```

`code-intelligence-mcp-server status` works for both install paths because it reads from the running process, not from a specific plist location.

The binary's `install` / lifecycle subcommands remain the supported flow for users who installed via npm or downloaded the binary directly.
