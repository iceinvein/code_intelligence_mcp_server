//! CLI argument parsing and help text.
//!
//! v4.0 introduces lifecycle subcommands (install, uninstall, start, stop,
//! status, migrate) alongside the default "run server" mode. When no
//! subcommand is given the binary boots the HTTP daemon.
//!
//! The legacy `--standalone` flag and `CIMCP_MODE` env var are accepted as
//! no-ops so v3 configs do not error.

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Run,
    Install(InstallOpts),
    Uninstall,
    Start,
    Stop,
    Status,
    Migrate(MigrateOpts),
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct InstallOpts {
    pub port: Option<u16>,
    pub no_autostart: bool,
    pub no_launchd: bool,
    /// Some(true): patch automatically, Some(false): never patch,
    /// None: ask the user interactively.
    pub patch_claude_json: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MigrateOpts {
    /// When true, only report what would change without writing.
    pub dry_run: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliArgs {
    pub help: bool,
    pub version: bool,
    pub command: Command,
    pub host: Option<String>,
    pub port: Option<u16>,
    pub discovery_port: Option<u16>,
}

impl Default for CliArgs {
    fn default() -> Self {
        Self {
            help: false,
            version: false,
            command: Command::Run,
            host: None,
            port: None,
            discovery_port: None,
        }
    }
}

pub fn parse_args(args: &[String]) -> CliArgs {
    let mut cli = CliArgs::default();
    let mut install_opts = InstallOpts::default();
    let mut migrate_opts = MigrateOpts::default();
    let mut subcommand: Option<&str> = None;

    let mut i = 1;
    while i < args.len() {
        let arg = args[i].as_str();
        match arg {
            "-h" | "--help" | "help" => cli.help = true,
            "-V" | "--version" | "version" => cli.version = true,
            // Subcommands. The first one wins; later positionals are flag values.
            "install" | "uninstall" | "start" | "stop" | "status" | "migrate"
                if subcommand.is_none() =>
            {
                subcommand = Some(match arg {
                    "install" => "install",
                    "uninstall" => "uninstall",
                    "start" => "start",
                    "stop" => "stop",
                    "status" => "status",
                    "migrate" => "migrate",
                    _ => unreachable!(),
                });
            }
            "--standalone" => {} // legacy no-op
            "--host" => {
                if i + 1 < args.len() {
                    cli.host = Some(args[i + 1].clone());
                    i += 1;
                }
            }
            "--port" => {
                if i + 1 < args.len() {
                    if let Ok(port) = args[i + 1].parse::<u16>() {
                        cli.port = Some(port);
                        install_opts.port = Some(port);
                    }
                    i += 1;
                }
            }
            "--discovery-port" => {
                if i + 1 < args.len() {
                    if let Ok(port) = args[i + 1].parse::<u16>() {
                        cli.discovery_port = Some(port);
                    }
                    i += 1;
                }
            }
            "--no-autostart" => install_opts.no_autostart = true,
            "--no-launchd" => install_opts.no_launchd = true,
            "--patch-claude-json" => install_opts.patch_claude_json = Some(true),
            "--no-patch-claude-json" => install_opts.patch_claude_json = Some(false),
            "--dry-run" => migrate_opts.dry_run = true,
            _ => {}
        }
        i += 1;
    }

    cli.command = match subcommand {
        Some("install") => Command::Install(install_opts),
        Some("uninstall") => Command::Uninstall,
        Some("start") => Command::Start,
        Some("stop") => Command::Stop,
        Some("status") => Command::Status,
        Some("migrate") => Command::Migrate(migrate_opts),
        _ => Command::Run,
    };

    cli
}

pub fn print_help() {
    println!("code-intelligence-mcp-server");
    println!();
    println!("HTTP MCP daemon for local code intelligence (index + search + context).");
    println!();
    println!("Usage:");
    println!("  code-intelligence-mcp-server [run]                Start the HTTP daemon");
    println!(
        "  code-intelligence-mcp-server install [opts]       Register and start the launchd daemon"
    );
    println!("  code-intelligence-mcp-server uninstall            Stop and unregister the launchd daemon");
    println!("  code-intelligence-mcp-server start                Start the registered daemon");
    println!("  code-intelligence-mcp-server stop                 Stop the registered daemon");
    println!(
        "  code-intelligence-mcp-server status               Show daemon state, port, version"
    );
    println!("  code-intelligence-mcp-server migrate [--dry-run]  Rewrite v3 stdio MCP configs to v4 HTTP");
    println!();
    println!("Run-mode flags:");
    println!("  --host HOST             Override listen address (default: 127.0.0.1)");
    println!("  --port PORT             Override listen port (default: 17800)");
    println!("  --discovery-port PORT   Discovery endpoint port (default: MCP port + 1)");
    println!();
    println!("install flags:");
    println!(
        "  --port PORT                 Pin the daemon port (default: 17800; auto-bumps if busy)"
    );
    println!("  --no-autostart              Do not start the daemon at login (KeepAlive still on)");
    println!("  --no-launchd                Write the plist but skip launchctl bootstrap");
    println!("  --patch-claude-json         Patch ~/.claude.json without prompting");
    println!("  --no-patch-claude-json      Skip the ~/.claude.json patch without prompting");
    println!();
    println!("Data location: ~/.code-intelligence/");
    println!("Configuration file: ~/.code-intelligence/server.toml");
    println!();
    println!("Common env (defaults shown):");
    println!("  EMBEDDINGS_BACKEND=llamacpp|hash      (default: llamacpp)");
    println!("  EMBEDDINGS_DEVICE=cpu|metal           (default: metal)");
    println!("  LLM_DEVICE=cpu|metal                  (default: cpu)");
    println!("  WATCH_MODE=true|false                 (default: true)");
}

pub fn print_version() {
    println!("{}", env!("CARGO_PKG_VERSION"));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_args_detects_help_flags() {
        assert!(parse_args(&["bin".into(), "--help".into()]).help);
        assert!(parse_args(&["bin".into(), "-h".into()]).help);
        assert!(parse_args(&["bin".into(), "help".into()]).help);
    }

    #[test]
    fn parse_args_detects_version_flags() {
        assert!(parse_args(&["bin".into(), "--version".into()]).version);
        assert!(parse_args(&["bin".into(), "-V".into()]).version);
    }

    #[test]
    fn parse_args_parses_host_and_port() {
        let cli = parse_args(&[
            "bin".into(),
            "--host".into(),
            "0.0.0.0".into(),
            "--port".into(),
            "4444".into(),
        ]);
        assert_eq!(cli.host.as_deref(), Some("0.0.0.0"));
        assert_eq!(cli.port, Some(4444));
    }

    #[test]
    fn parse_args_defaults_to_run() {
        let cli = parse_args(&["bin".into()]);
        assert_eq!(cli.command, Command::Run);
    }

    #[test]
    fn parse_args_recognises_install_subcommand() {
        let cli = parse_args(&["bin".into(), "install".into()]);
        match cli.command {
            Command::Install(opts) => {
                assert_eq!(opts.port, None);
                assert!(!opts.no_autostart);
                assert_eq!(opts.patch_claude_json, None);
            }
            _ => panic!("expected install command, got {:?}", cli.command),
        }
    }

    #[test]
    fn parse_args_install_with_flags() {
        let cli = parse_args(&[
            "bin".into(),
            "install".into(),
            "--port".into(),
            "20000".into(),
            "--no-autostart".into(),
            "--patch-claude-json".into(),
        ]);
        match cli.command {
            Command::Install(opts) => {
                assert_eq!(opts.port, Some(20000));
                assert!(opts.no_autostart);
                assert_eq!(opts.patch_claude_json, Some(true));
            }
            _ => panic!("expected install"),
        }
    }

    #[test]
    fn parse_args_no_patch_flag_is_distinct() {
        let cli = parse_args(&[
            "bin".into(),
            "install".into(),
            "--no-patch-claude-json".into(),
        ]);
        match cli.command {
            Command::Install(opts) => assert_eq!(opts.patch_claude_json, Some(false)),
            _ => panic!("expected install"),
        }
    }

    #[test]
    fn parse_args_migrate_with_dry_run() {
        let cli = parse_args(&["bin".into(), "migrate".into(), "--dry-run".into()]);
        match cli.command {
            Command::Migrate(opts) => assert!(opts.dry_run),
            _ => panic!("expected migrate"),
        }
    }

    #[test]
    fn parse_args_silently_accepts_legacy_standalone_flag() {
        let cli = parse_args(&["bin".into(), "--standalone".into()]);
        assert_eq!(cli.command, Command::Run);
    }

    #[test]
    fn parse_args_ignores_unknown_args() {
        let cli = parse_args(&["bin".into(), "--unknown".into()]);
        assert_eq!(cli.command, Command::Run);
    }

    #[test]
    fn parse_args_parses_discovery_port() {
        let cli = parse_args(&["bin".into(), "--discovery-port".into(), "5000".into()]);
        assert_eq!(cli.discovery_port, Some(5000));
    }
}
