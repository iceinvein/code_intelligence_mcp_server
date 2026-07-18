//! Install / uninstall / status subcommands for the v4 daemon.
//!
//! Manages the launchd plist at
//! `~/Library/LaunchAgents/com.iceinvein.code-intelligence.plist`, the matching
//! `~/.claude.json` mcpServers entry, and the daemon process lifecycle via
//! `launchctl bootstrap` / `bootout` / `kickstart`.

use anyhow::{anyhow, Context, Result};
use std::fs;
use std::io::{self, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::cli::{InstallOpts, MigrateOpts};
use crate::config::StandaloneConfig;

pub const LABEL: &str = "com.iceinvein.code-intelligence";
pub const DEFAULT_PORT: u16 = 17800;
pub const MCP_SERVER_NAME: &str = "code-intelligence";

// ---------- Path helpers ----------

fn home_dir() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| anyhow!("HOME is not set"))
}

fn plist_path() -> Result<PathBuf> {
    Ok(home_dir()?
        .join("Library/LaunchAgents")
        .join(format!("{LABEL}.plist")))
}

fn launch_agents_dir() -> Result<PathBuf> {
    Ok(home_dir()?.join("Library/LaunchAgents"))
}

fn data_dir() -> Result<PathBuf> {
    Ok(home_dir()?.join(".code-intelligence"))
}

fn logs_dir() -> Result<PathBuf> {
    Ok(data_dir()?.join("logs"))
}

fn claude_json_path() -> Result<PathBuf> {
    Ok(home_dir()?.join(".claude.json"))
}

fn current_uid() -> u32 {
    // SAFETY: getuid is documented to always succeed and has no side effects.
    unsafe { libc::getuid() }
}

fn service_target() -> String {
    format!("gui/{}/{LABEL}", current_uid())
}

fn domain_target() -> String {
    format!("gui/{}", current_uid())
}

// ---------- macOS version guard ----------

fn require_macos_13_plus() -> Result<()> {
    let out = Command::new("sw_vers")
        .arg("-productVersion")
        .output()
        .context("running sw_vers to read macOS version")?;
    let version = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let major: u32 = version
        .split('.')
        .next()
        .unwrap_or("0")
        .parse()
        .unwrap_or(0);
    if major < 13 {
        return Err(anyhow!(
            "Install requires macOS 13 (Ventura) or later for `launchctl bootstrap`. \
             Detected macOS {version}. Use `launchctl load` manually if you must."
        ));
    }
    Ok(())
}

// ---------- Port helpers ----------

fn port_is_free(port: u16) -> bool {
    TcpListener::bind(("127.0.0.1", port)).is_ok()
}

/// Find the first free port at or after `start`, scanning up to 100 ports.
fn pick_port(start: u16) -> Result<u16> {
    for offset in 0..100u16 {
        let candidate = start.saturating_add(offset);
        if candidate == 0 {
            break;
        }
        if port_is_free(candidate) {
            return Ok(candidate);
        }
    }
    Err(anyhow!(
        "Could not find a free TCP port in range {start}-{}",
        start.saturating_add(99)
    ))
}

// ---------- Daemon state detection ----------

/// What the `status` subcommand concluded about a running daemon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DaemonState {
    /// Nothing is serving the port and our launchd label is not loaded.
    Stopped,
    /// Running under our launchd label (`com.iceinvein.code-intelligence`).
    Running,
    /// A daemon is serving the port, but not via our launchd label, e.g.
    /// Homebrew services (`homebrew.mxcl.code-intelligence-mcp`) or a process
    /// started by hand. Reported so `status` never claims "stopped" while the
    /// port is plainly in use.
    RunningUnmanaged,
}

/// Decide daemon state from two independent signals: whether our launchd label
/// reports the service loaded, and whether the MCP port is in use by anything.
///
/// The port probe is the catch-all that fixes the long-standing false-"stopped"
/// report: `launchctl print` only knows about our own label, so a daemon
/// supervised by Homebrew (a different label) or launched directly used to read
/// as "stopped" even while it was serving requests.
fn determine_daemon_state(label_running: bool, port_in_use: bool) -> DaemonState {
    if label_running {
        DaemonState::Running
    } else if port_in_use {
        DaemonState::RunningUnmanaged
    } else {
        DaemonState::Stopped
    }
}

fn render_producer_summary(
    producers: &[crate::external_index::manifest::ProducerAvailability],
    auto_enabled: bool,
) -> String {
    let integrated = producers
        .iter()
        .filter(|producer| producer.availability == "bundled")
        .count();
    let adapters = producers
        .iter()
        .filter(|producer| producer.readiness == "adapter_only")
        .count();
    let policy = if auto_enabled {
        "auto indexing enabled"
    } else {
        "auto indexing disabled"
    };
    format!(
        "External producers: integrated {integrated}/{}, adapter contracts {adapters}, {policy}",
        producers.len(),
    )
}

fn external_index_auto_enabled() -> bool {
    StandaloneConfig::load(None, None, None)
        .map(|cfg| cfg.external_index_auto)
        .unwrap_or(false)
}

// ---------- plist template ----------

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn render_plist(self_path: &Path, port: u16, autostart: bool, home: &Path) -> String {
    let logs = home.join(".code-intelligence/logs");
    let stdout_log = logs.join("launchd.out.log");
    let stderr_log = logs.join("launchd.err.log");
    let self_str = xml_escape(&self_path.display().to_string());
    let home_str = xml_escape(&home.display().to_string());
    let stdout_str = xml_escape(&stdout_log.display().to_string());
    let stderr_str = xml_escape(&stderr_log.display().to_string());
    let autostart_bool = if autostart { "true" } else { "false" };
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>{LABEL}</string>

  <key>ProgramArguments</key>
  <array>
    <string>{self_str}</string>
    <string>--port</string>
    <string>{port}</string>
  </array>

  <key>EnvironmentVariables</key>
  <dict>
    <key>HOME</key>
    <string>{home}</string>
    <key>LLM_DEVICE</key>
    <string>metal</string>
    <key>EMBEDDINGS_DEVICE</key>
    <string>metal</string>
    <key>RUST_LOG</key>
    <string>info</string>
  </dict>

  <key>RunAtLoad</key>
  <{autostart_bool}/>

  <key>KeepAlive</key>
  <true/>

  <key>ThrottleInterval</key>
  <integer>30</integer>

  <key>ProcessType</key>
  <string>Interactive</string>

  <key>StandardOutPath</key>
  <string>{stdout}</string>

  <key>StandardErrorPath</key>
  <string>{stderr}</string>
</dict>
</plist>
"#,
        home = home_str,
        stdout = stdout_str,
        stderr = stderr_str,
    )
}

fn write_plist(content: &str) -> Result<PathBuf> {
    let dir = launch_agents_dir()?;
    fs::create_dir_all(&dir).context("creating LaunchAgents dir")?;
    let plist = plist_path()?;
    atomic_write(&plist, content.as_bytes()).context("writing plist")?;
    Ok(plist)
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("path has no parent: {}", path.display()))?;
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmp = parent.join(format!(
        ".{}.tmp.{ts}.{pid}",
        path.file_name().and_then(|n| n.to_str()).unwrap_or("write"),
        pid = std::process::id()
    ));
    {
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)
            .with_context(|| format!("creating tempfile {}", tmp.display()))?;
        f.write_all(bytes).context("writing tempfile")?;
        f.flush().context("flushing tempfile")?;
    }
    fs::rename(&tmp, path)
        .with_context(|| format!("renaming {} -> {}", tmp.display(), path.display()))?;
    Ok(())
}

// ---------- launchctl wrappers ----------

fn launchctl_running() -> Result<bool> {
    let out = Command::new("launchctl")
        .args(["print", &service_target()])
        .output()
        .context("invoking launchctl print")?;
    Ok(out.status.success())
}

fn launchctl_bootstrap(plist: &Path) -> Result<()> {
    let status = Command::new("launchctl")
        .args(["bootstrap", &domain_target()])
        .arg(plist)
        .status()
        .context("invoking launchctl bootstrap")?;
    if !status.success() {
        return Err(anyhow!("launchctl bootstrap failed with status {}", status));
    }
    Ok(())
}

fn launchctl_bootout() -> Result<()> {
    let status = Command::new("launchctl")
        .args(["bootout", &service_target()])
        .status()
        .context("invoking launchctl bootout")?;
    if !status.success() {
        return Err(anyhow!("launchctl bootout failed with status {}", status));
    }
    Ok(())
}

fn launchctl_kickstart() -> Result<()> {
    let status = Command::new("launchctl")
        .args(["kickstart", "-k", &service_target()])
        .status()
        .context("invoking launchctl kickstart")?;
    if !status.success() {
        return Err(anyhow!("launchctl kickstart failed with status {}", status));
    }
    Ok(())
}

// ---------- Public subcommand entry points ----------

pub fn handle_install(opts: InstallOpts) -> Result<()> {
    require_macos_13_plus()?;

    let self_path = std::env::current_exe().context("resolving self path via current_exe")?;
    let preferred_port = opts.port.unwrap_or(DEFAULT_PORT);

    // If the service is already running, tear it down before re-bootstrapping
    // so the new plist takes effect.
    if launchctl_running().unwrap_or(false) {
        eprintln!("Existing daemon detected; stopping it first...");
        let _ = launchctl_bootout();
    }

    let port = if port_is_free(preferred_port) {
        preferred_port
    } else {
        let alt = pick_port(preferred_port.saturating_add(1))?;
        eprintln!("Port {preferred_port} is busy; using {alt} instead. Pass `--port` to override.");
        alt
    };

    // Ensure data + logs dirs exist before launchd writes there.
    fs::create_dir_all(logs_dir()?).context("creating logs dir")?;
    fs::create_dir_all(data_dir()?.join("repos")).context("creating repos dir")?;

    let plist = render_plist(&self_path, port, !opts.no_autostart, &home_dir()?);
    let plist_p = write_plist(&plist)?;
    println!("Wrote {}", plist_p.display());

    if opts.no_launchd {
        println!("--no-launchd set; not invoking launchctl. Bootstrap manually with:");
        println!(
            "  launchctl bootstrap {} {}",
            domain_target(),
            plist_p.display()
        );
    } else {
        launchctl_bootstrap(&plist_p)?;
        println!("Daemon registered with launchd (label: {LABEL}, port: {port})");
    }

    let producers = crate::external_index::manifest::producer_availability().unwrap_or_default();
    println!(
        "{}",
        render_producer_summary(&producers, external_index_auto_enabled())
    );

    let want_patch = match opts.patch_claude_json {
        Some(b) => b,
        None => prompt_yes_no(
            &format!(
                "Patch ~/.claude.json so MCP clients connect at http://127.0.0.1:{port}/mcp? [Y/n] "
            ),
            true,
        ),
    };

    if want_patch {
        match patch_claude_json(port) {
            Ok(ClaudePatchResult::Patched { backup }) => {
                println!("Patched ~/.claude.json (backup at {})", backup.display());
            }
            Ok(ClaudePatchResult::Created) => {
                println!("Created ~/.claude.json with code-intelligence MCP entry.");
            }
            Ok(ClaudePatchResult::Unchanged) => {
                println!("~/.claude.json already up to date; no changes made.");
            }
            Err(e) => eprintln!("Warning: could not patch ~/.claude.json: {e}"),
        }
    } else {
        println!("Skipped ~/.claude.json patch. Configure your MCP clients to point at:");
        println!("  http://127.0.0.1:{port}/mcp");
    }

    println!();
    println!("Done. Tail logs with:");
    println!(
        "  tail -f {}",
        logs_dir()?.join("launchd.err.log").display()
    );
    println!("Stop the daemon with:");
    println!("  {} stop", self_path.display());
    Ok(())
}

pub fn handle_uninstall() -> Result<()> {
    let plist_p = plist_path()?;
    if launchctl_running().unwrap_or(false) {
        match launchctl_bootout() {
            Ok(()) => println!("Stopped and unregistered the daemon."),
            Err(e) => eprintln!("Warning: launchctl bootout failed: {e}"),
        }
    } else {
        println!("Daemon was not running.");
    }
    if plist_p.exists() {
        fs::remove_file(&plist_p).context("removing plist")?;
        println!("Removed {}", plist_p.display());
    }
    println!();
    println!("Note: ~/.claude.json is left untouched. Restore from a backup if needed:");
    println!("  ls ~/.claude.json.bak.* 2>/dev/null");
    Ok(())
}

pub fn handle_start() -> Result<()> {
    require_macos_13_plus()?;
    let plist_p = plist_path()?;
    if !plist_p.exists() {
        return Err(anyhow!(
            "No plist at {}. Run `install` first.",
            plist_p.display()
        ));
    }
    if launchctl_running().unwrap_or(false) {
        launchctl_kickstart()?;
        println!("Kickstarted {LABEL}.");
    } else {
        launchctl_bootstrap(&plist_p)?;
        println!("Bootstrapped {LABEL}.");
    }
    Ok(())
}

pub fn handle_stop() -> Result<()> {
    if launchctl_running().unwrap_or(false) {
        launchctl_bootout()?;
        println!("Stopped {LABEL}.");
    } else {
        println!("{LABEL} is not running.");
    }
    Ok(())
}

pub fn handle_status() -> Result<()> {
    let plist_p = plist_path()?;
    let installed = plist_p.exists();
    let label_running = launchctl_running().unwrap_or(false);
    // Probe the port too: `launchctl print` only knows about our own label, so
    // a daemon supervised by Homebrew or started by hand would otherwise read
    // as "stopped" while it is plainly serving requests.
    let port_in_use = !port_is_free(DEFAULT_PORT);
    let state = determine_daemon_state(label_running, port_in_use);

    println!(
        "code-intelligence-mcp-server v{}",
        env!("CARGO_PKG_VERSION")
    );
    println!(
        "  plist:     {} ({})",
        plist_p.display(),
        if installed { "present" } else { "missing" }
    );
    let daemon_line = match state {
        DaemonState::Running => "running".to_string(),
        DaemonState::RunningUnmanaged => format!(
            "running (port {DEFAULT_PORT} in use; not managed by our launchd label, \
             likely Homebrew services or a bare process)"
        ),
        DaemonState::Stopped => "stopped".to_string(),
    };
    println!("  daemon:    {daemon_line}");

    // PID via `launchctl print` is only meaningful when our own label is loaded.
    if state == DaemonState::Running {
        let out = Command::new("launchctl")
            .args(["print", &service_target()])
            .output()
            .context("invoking launchctl print")?;
        let text = String::from_utf8_lossy(&out.stdout);
        if let Some(pid_line) = text.lines().find(|l| l.trim_start().starts_with("pid =")) {
            println!("  {}", pid_line.trim());
        }
    }

    println!("  data dir:  {}", data_dir()?.display());
    println!("  logs:      {}", logs_dir()?.display());
    let producers = crate::external_index::manifest::producer_availability().unwrap_or_default();
    println!(
        "{}",
        render_producer_summary(&producers, external_index_auto_enabled())
    );
    Ok(())
}

// ---------- ~/.claude.json patcher ----------

/// Patches the `code-intelligence` entry in `~/.claude.json` to point at the
/// HTTP daemon.
pub enum ClaudePatchResult {
    Created,
    Patched { backup: PathBuf },
    Unchanged,
}

pub fn patch_claude_json(port: u16) -> Result<ClaudePatchResult> {
    let path = claude_json_path()?;
    if !path.exists() {
        // No file to patch. Create a minimal one so subsequent Claude Code
        // launches pick up our daemon entry.
        let new_content = serde_json::json!({
            "mcpServers": {
                MCP_SERVER_NAME: {
                    "type": "streamable-http",
                    "url": format!("http://127.0.0.1:{port}/mcp"),
                    "alwaysAllow": [],
                }
            }
        });
        atomic_write(
            &path,
            serde_json::to_string_pretty(&new_content)?.as_bytes(),
        )?;
        return Ok(ClaudePatchResult::Created);
    }

    let raw = fs::read_to_string(&path).context("reading ~/.claude.json")?;
    let mut value: serde_json::Value =
        serde_json::from_str(&raw).context("parsing ~/.claude.json")?;

    let desired = serde_json::json!({
        "type": "streamable-http",
        "url": format!("http://127.0.0.1:{port}/mcp"),
        "alwaysAllow": [],
    });

    let mcp = value
        .as_object_mut()
        .ok_or_else(|| anyhow!("~/.claude.json is not a JSON object"))?
        .entry("mcpServers")
        .or_insert_with(|| serde_json::json!({}));

    let mcp_obj = mcp
        .as_object_mut()
        .ok_or_else(|| anyhow!("~/.claude.json mcpServers is not an object"))?;

    // No-op if the entry already matches.
    if let Some(existing) = mcp_obj.get(MCP_SERVER_NAME) {
        let existing_url = existing.get("url").and_then(|v| v.as_str());
        let existing_type = existing.get("type").and_then(|v| v.as_str());
        let target_url = format!("http://127.0.0.1:{port}/mcp");
        if existing_type == Some("streamable-http") && existing_url == Some(target_url.as_str()) {
            return Ok(ClaudePatchResult::Unchanged);
        }
    }

    mcp_obj.insert(MCP_SERVER_NAME.to_string(), desired);

    let backup = backup_path(&path);
    fs::copy(&path, &backup).context("backing up ~/.claude.json")?;
    prune_backups(&path, 3).ok();

    let serialized = serde_json::to_string_pretty(&value)?;
    atomic_write(&path, serialized.as_bytes())?;
    Ok(ClaudePatchResult::Patched { backup })
}

fn backup_path(target: &Path) -> PathBuf {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let mut p = target.to_path_buf();
    let name = format!(
        "{}.bak.{}",
        target.file_name().and_then(|n| n.to_str()).unwrap_or(""),
        ts
    );
    p.set_file_name(name);
    p
}

fn prune_backups(target: &Path, keep: usize) -> Result<()> {
    let dir = target
        .parent()
        .ok_or_else(|| anyhow!("target has no parent"))?;
    let prefix = format!(
        "{}.bak.",
        target.file_name().and_then(|n| n.to_str()).unwrap_or("")
    );
    let mut backups: Vec<PathBuf> = fs::read_dir(dir)
        .context("listing backup dir")?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with(&prefix))
                .unwrap_or(false)
        })
        .collect();
    backups.sort();
    while backups.len() > keep {
        let oldest = backups.remove(0);
        let _ = fs::remove_file(oldest);
    }
    Ok(())
}

// ---------- migrate ----------

/// Find and rewrite v3-style code-intelligence stdio entries in ~/.claude.json.
pub fn handle_migrate(opts: MigrateOpts) -> Result<()> {
    let path = claude_json_path()?;
    if !path.exists() {
        println!("~/.claude.json does not exist; nothing to migrate.");
        return Ok(());
    }
    let raw = fs::read_to_string(&path).context("reading ~/.claude.json")?;
    let mut value: serde_json::Value = serde_json::from_str(&raw)?;

    // Determine the port from an existing patched entry, falling back to default.
    let port = extract_existing_port(&value).unwrap_or(DEFAULT_PORT);
    let target_url = format!("http://127.0.0.1:{port}/mcp");

    let mut rewrites: Vec<String> = Vec::new();
    rewrite_mcp_servers(&mut value, &target_url, &mut rewrites, "$.mcpServers");

    if let Some(projects) = value.get_mut("projects").and_then(|v| v.as_object_mut()) {
        for (proj_path, proj_val) in projects.iter_mut() {
            rewrite_mcp_servers(
                proj_val,
                &target_url,
                &mut rewrites,
                &format!("$.projects[{proj_path:?}].mcpServers"),
            );
        }
    }

    if rewrites.is_empty() {
        println!("No stale code-intelligence stdio entries found in ~/.claude.json.");
        return Ok(());
    }

    println!("Stale entries that will be rewritten to {target_url}:");
    for r in &rewrites {
        println!("  {r}");
    }

    if opts.dry_run {
        println!("\nDry run; no changes written. Run without --dry-run to apply.");
        return Ok(());
    }

    let backup = backup_path(&path);
    fs::copy(&path, &backup).context("backing up ~/.claude.json")?;
    prune_backups(&path, 3).ok();
    let serialized = serde_json::to_string_pretty(&value)?;
    atomic_write(&path, serialized.as_bytes())?;
    println!("\nApplied. Backup at {}", backup.display());
    Ok(())
}

fn extract_existing_port(value: &serde_json::Value) -> Option<u16> {
    let url = value
        .get("mcpServers")?
        .get(MCP_SERVER_NAME)?
        .get("url")?
        .as_str()?;
    let after_colon = url.rsplit(':').next()?;
    let port_str: String = after_colon
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    port_str.parse().ok()
}

fn rewrite_mcp_servers(
    container: &mut serde_json::Value,
    target_url: &str,
    rewrites: &mut Vec<String>,
    path_label: &str,
) {
    let Some(mcp) = container
        .get_mut("mcpServers")
        .and_then(|v| v.as_object_mut())
    else {
        return;
    };
    let Some(entry) = mcp.get_mut(MCP_SERVER_NAME) else {
        return;
    };
    let is_stdio = entry.get("command").is_some()
        || entry.get("type").and_then(|v| v.as_str()) == Some("stdio");
    if !is_stdio {
        return;
    }
    rewrites.push(path_label.to_string());
    let always_allow = entry
        .get("alwaysAllow")
        .cloned()
        .unwrap_or_else(|| serde_json::json!([]));
    *entry = serde_json::json!({
        "type": "streamable-http",
        "url": target_url,
        "alwaysAllow": always_allow,
    });
}

// ---------- Interactive prompt ----------

fn prompt_yes_no(prompt: &str, default_yes: bool) -> bool {
    print!("{prompt}");
    let _ = io::stdout().flush();
    let mut s = String::new();
    if io::stdin().read_line(&mut s).is_err() {
        return default_yes;
    }
    match s.trim().to_lowercase().as_str() {
        "" => default_yes,
        "y" | "yes" => true,
        "n" | "no" => false,
        _ => default_yes,
    }
}

// ---------- Tests ----------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_plist_contains_self_path_and_port() {
        let path = PathBuf::from("/opt/local/bin/code-intelligence-mcp-server");
        let plist = render_plist(&path, 18000, true, Path::new("/Users/test"));
        assert!(plist.contains("/opt/local/bin/code-intelligence-mcp-server"));
        assert!(plist.contains("<string>18000</string>"));
        assert!(plist.contains("<key>RunAtLoad</key>\n  <true/>"));
        assert!(plist.contains("<key>Label</key>"));
        assert!(plist.contains(LABEL));
        assert!(plist.contains("/Users/test/.code-intelligence/logs/launchd.err.log"));
    }

    #[test]
    fn render_plist_respects_no_autostart() {
        let plist = render_plist(Path::new("/x"), 18000, false, Path::new("/Users/test"));
        assert!(plist.contains("<key>RunAtLoad</key>\n  <false/>"));
    }

    #[test]
    fn producer_summary_counts_available_and_missing() {
        let producers = vec![
            crate::external_index::manifest::ProducerAvailability {
                id: "rust".to_string(),
                language: "rust".to_string(),
                tier: "first_class".to_string(),
                readiness: "integrated".to_string(),
                executable: "/bin/code-intelligence-external-rust".to_string(),
                availability: "bundled".to_string(),
            },
            crate::external_index::manifest::ProducerAvailability {
                id: "python".to_string(),
                language: "python".to_string(),
                tier: "first_class".to_string(),
                readiness: "integrated".to_string(),
                executable: "code-intelligence-external-python".to_string(),
                availability: "missing".to_string(),
            },
            crate::external_index::manifest::ProducerAvailability {
                id: "java".to_string(),
                language: "java".to_string(),
                tier: "build_aware".to_string(),
                readiness: "adapter_only".to_string(),
                executable: "code-intelligence-external-java".to_string(),
                availability: "adapter_only".to_string(),
            },
        ];

        assert_eq!(
            render_producer_summary(&producers, false),
            "External producers: integrated 1/3, adapter contracts 1, auto indexing disabled"
        );
        assert_eq!(
            render_producer_summary(&producers, true),
            "External producers: integrated 1/3, adapter contracts 1, auto indexing enabled"
        );
    }

    #[test]
    fn render_plist_escapes_xml_values() {
        let plist = render_plist(
            Path::new("/Users/a&b/bin/<server>"),
            18000,
            true,
            Path::new("/Users/a&b"),
        );
        assert!(plist.contains("/Users/a&amp;b/bin/&lt;server&gt;"));
        assert!(plist.contains("/Users/a&amp;b/.code-intelligence/logs/launchd.err.log"));
    }

    #[test]
    fn daemon_state_running_when_our_label_is_loaded() {
        // Our launchd label being live is authoritative regardless of the port
        // probe outcome.
        assert_eq!(determine_daemon_state(true, true), DaemonState::Running);
        assert_eq!(determine_daemon_state(true, false), DaemonState::Running);
    }

    #[test]
    fn daemon_state_unmanaged_when_port_busy_but_label_absent() {
        // Regression: a daemon supervised by a *different* launchd label
        // (Homebrew's `homebrew.mxcl.code-intelligence-mcp`) or started bare
        // must never be reported as "stopped" while it is plainly serving the
        // port. This is the false-"stopped" bug `status` used to print.
        assert_eq!(
            determine_daemon_state(false, true),
            DaemonState::RunningUnmanaged
        );
    }

    #[test]
    fn daemon_state_stopped_only_when_nothing_listens() {
        assert_eq!(determine_daemon_state(false, false), DaemonState::Stopped);
    }

    #[test]
    fn pick_port_returns_default_when_free() {
        // Pick a fresh port using OS assignment so the test does not flake.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let busy = listener.local_addr().unwrap().port();
        // The port is busy because we still hold the listener; pick_port
        // should walk forward.
        let alt = pick_port(busy).unwrap();
        assert_ne!(alt, busy);
    }

    #[test]
    fn extract_existing_port_parses_url() {
        let v = serde_json::json!({
            "mcpServers": {
                "code-intelligence": {
                    "type": "streamable-http",
                    "url": "http://127.0.0.1:18000/mcp"
                }
            }
        });
        assert_eq!(extract_existing_port(&v), Some(18000));
    }

    #[test]
    fn rewrite_mcp_servers_replaces_stdio_entry() {
        let mut v = serde_json::json!({
            "mcpServers": {
                "code-intelligence": {
                    "command": "npx",
                    "args": ["-y", "@iceinvein/code-intelligence-mcp"],
                    "alwaysAllow": ["search_code"]
                }
            }
        });
        let mut rewrites = Vec::new();
        rewrite_mcp_servers(
            &mut v,
            "http://127.0.0.1:18000/mcp",
            &mut rewrites,
            "$.mcpServers",
        );
        assert_eq!(rewrites.len(), 1);
        let entry = &v["mcpServers"]["code-intelligence"];
        assert_eq!(entry["type"], "streamable-http");
        assert_eq!(entry["url"], "http://127.0.0.1:18000/mcp");
        assert_eq!(entry["alwaysAllow"], serde_json::json!(["search_code"]));
        assert!(entry.get("command").is_none());
    }

    #[test]
    fn rewrite_mcp_servers_leaves_http_entry_alone() {
        let mut v = serde_json::json!({
            "mcpServers": {
                "code-intelligence": {
                    "type": "streamable-http",
                    "url": "http://127.0.0.1:17800/mcp"
                }
            }
        });
        let mut rewrites = Vec::new();
        rewrite_mcp_servers(
            &mut v,
            "http://127.0.0.1:18000/mcp",
            &mut rewrites,
            "$.mcpServers",
        );
        assert!(rewrites.is_empty());
    }

    #[test]
    fn backup_path_appends_timestamp() {
        let p = backup_path(Path::new("/tmp/.claude.json"));
        let name = p.file_name().unwrap().to_string_lossy().to_string();
        assert!(name.starts_with(".claude.json.bak."));
        let suffix = name.trim_start_matches(".claude.json.bak.");
        assert!(
            suffix.parse::<u64>().is_ok(),
            "suffix not a unix ts: {suffix}"
        );
    }
}
