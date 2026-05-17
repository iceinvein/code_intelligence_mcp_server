# typed: false
# frozen_string_literal: true

# This file is the canonical source for the Homebrew formula. The
# `iceinvein/homebrew-tap` repository hosts a copy. Each release updates
# the version, URL, and sha256, and pushes both copies together.
#
# The formula is intentionally minimal:
#   * Downloads the prebuilt arm64 binary tarball from the GitHub release
#     that matches `version`.
#   * Installs the single binary into the Homebrew prefix.
#   * Declares a service so users can `brew services start
#     code-intelligence-mcp` and let launchd manage the daemon.
#
# Users on this distribution path do NOT run the binary's `install`
# subcommand. Brew owns the launchd plist; the binary's `install` /
# `uninstall` / `start` / `stop` / `status` subcommands are for users
# who installed via npm or downloaded the binary directly.

class CodeIntelligenceMcp < Formula
  desc "Local code intelligence MCP server with semantic search, graph navigation, on-device LLM"
  homepage "https://github.com/iceinvein/code_intelligence_mcp_server"
  version "4.0.4"
  license "MIT"

  on_macos do
    on_arm do
      url "https://github.com/iceinvein/code_intelligence_mcp_server/releases/download/v#{version}/code-intelligence-mcp-server-aarch64-apple-darwin.tar.gz"
      # The `sha256` is rewritten by `scripts/release.sh` (or the release
      # workflow's bump step) after the tarball is built and uploaded.
      sha256 "c584535bcfbe092050ea9500ad3f42b41f02f67ccf74a8c48a17d9c84fc8b140"
    end

    on_intel do
      odie "code-intelligence-mcp requires an Apple Silicon Mac. Intel macOS is not supported."
    end
  end

  on_linux do
    odie "code-intelligence-mcp is macOS-only (Apple Silicon)."
  end

  def install
    bin.install "code-intelligence-mcp-server"
  end

  service do
    run [opt_bin/"code-intelligence-mcp-server"]
    keep_alive true
    process_type :background
    log_path var/"log/code-intelligence-mcp.out.log"
    error_log_path var/"log/code-intelligence-mcp.err.log"
    working_dir HOMEBREW_PREFIX
  end

  test do
    # The binary should respond to --help without needing models. We can't
    # exercise the daemon in `brew test` because it would race with any
    # already-running service on the same port and download multi-GB
    # model weights.
    assert_match "code-intelligence-mcp-server", shell_output("#{bin}/code-intelligence-mcp-server --help 2>&1", 0)
  end

  def caveats
    <<~EOS
      The daemon listens on http://127.0.0.1:17800/mcp once started.
      Dashboard:    http://127.0.0.1:17802/
      Logs:         brew services log code-intelligence-mcp

      To start the daemon:
        brew services start code-intelligence-mcp

      Bind a workspace by configuring your MCP client URL as
        http://127.0.0.1:17800/mcp?repo=/abs/path/to/your/repo

      Or use `code-intelligence-mcp-server migrate` to rewrite an existing
      v3 ~/.claude.json entry to the new daemon URL.

      First start downloads ~3.2 GB of GGUF models to
      ~/.code-intelligence/models/ (one-time).
    EOS
  end
end
