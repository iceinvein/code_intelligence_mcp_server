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
    InstallAgent(AgentInstallOpts),
    UninstallAgent(AgentUninstallOpts),
    Search(SearchOpts),
    Investigate(InvestigateOpts),
    Ask(AskOpts),
    Hydrate(HydrateOpts),
    RepoMap(RepoMapOpts),
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
pub struct AgentInstallOpts {
    pub targets: Vec<String>,
    pub scope: String,
    pub repo: Option<String>,
    pub port: Option<u16>,
    pub print_config: bool,
    pub dry_run: bool,
    pub yes: bool,
    pub no_instructions: bool,
    pub no_mcp: bool,
}

impl Default for AgentInstallOpts {
    fn default() -> Self {
        Self {
            targets: vec!["auto".to_string()],
            scope: "project".to_string(),
            repo: None,
            port: None,
            print_config: false,
            dry_run: false,
            yes: false,
            no_instructions: false,
            no_mcp: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentUninstallOpts {
    pub targets: Vec<String>,
    pub scope: String,
    pub repo: Option<String>,
    pub dry_run: bool,
    pub yes: bool,
    pub no_instructions: bool,
    pub no_mcp: bool,
}

impl Default for AgentUninstallOpts {
    fn default() -> Self {
        Self {
            targets: vec!["auto".to_string()],
            scope: "project".to_string(),
            repo: None,
            dry_run: false,
            yes: false,
            no_instructions: false,
            no_mcp: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SearchOpts {
    pub repo: Option<String>,
    pub query: String,
    pub limit: Option<u32>,
    pub context: Option<String>,
    pub json: bool,
    pub pretty: bool,
    pub timeout: Option<String>,
    pub no_start: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct InvestigateOpts {
    pub repo: Option<String>,
    pub question: String,
    pub target: Option<String>,
    pub file_path: Option<String>,
    pub mode: Option<String>,
    pub max_hops: Option<u32>,
    pub json: bool,
    pub pretty: bool,
    pub timeout: Option<String>,
    pub no_start: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AskOpts {
    pub repo: Option<String>,
    pub question: String,
    pub target: Option<String>,
    pub file_path: Option<String>,
    pub mode: Option<String>,
    pub max_evidence: Option<u32>,
    pub quality: Option<String>,
    pub json: bool,
    pub pretty: bool,
    pub timeout: Option<String>,
    pub no_start: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HydrateOpts {
    pub repo: Option<String>,
    pub ids: Vec<String>,
    pub mode: Option<String>,
    pub verbose: bool,
    pub json: bool,
    pub pretty: bool,
    pub timeout: Option<String>,
    pub no_start: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RepoMapOpts {
    pub repo: Option<String>,
    pub budget: Option<u32>,
    pub max_files: Option<u32>,
    pub max_symbols_per_file: Option<u32>,
    pub json: bool,
    pub pretty: bool,
    pub timeout: Option<String>,
    pub no_start: bool,
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
    let mut agent_install_opts = AgentInstallOpts::default();
    let mut agent_uninstall_opts = AgentUninstallOpts::default();
    let mut search_opts = SearchOpts::default();
    let mut investigate_opts = InvestigateOpts::default();
    let mut ask_opts = AskOpts::default();
    let mut hydrate_opts = HydrateOpts::default();
    let mut repo_map_opts = RepoMapOpts::default();
    let mut subcommand: Option<&str> = None;
    let mut query_words: Vec<String> = Vec::new();

    let mut i = 1;
    while i < args.len() {
        let arg = args[i].as_str();
        match arg {
            "-h" | "--help" | "help" => cli.help = true,
            "-V" | "--version" | "version" => cli.version = true,
            // Subcommands. The first one wins; later positionals are flag values.
            "install" | "uninstall" | "start" | "stop" | "status" | "migrate" | "install-agent"
            | "uninstall-agent" | "search" | "investigate" | "ask" | "hydrate" | "repo-map"
                if subcommand.is_none() =>
            {
                subcommand = Some(match arg {
                    "install" => "install",
                    "uninstall" => "uninstall",
                    "start" => "start",
                    "stop" => "stop",
                    "status" => "status",
                    "migrate" => "migrate",
                    "install-agent" => "install-agent",
                    "uninstall-agent" => "uninstall-agent",
                    "search" => "search",
                    "investigate" => "investigate",
                    "ask" => "ask",
                    "hydrate" => "hydrate",
                    "repo-map" => "repo-map",
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
                        agent_install_opts.port = Some(port);
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
            "--dry-run" => match subcommand {
                Some("install-agent") => agent_install_opts.dry_run = true,
                Some("uninstall-agent") => agent_uninstall_opts.dry_run = true,
                _ => migrate_opts.dry_run = true,
            },
            "--repo"
                if matches!(
                    subcommand,
                    Some(
                        "install-agent"
                            | "uninstall-agent"
                            | "search"
                            | "investigate"
                            | "ask"
                            | "hydrate"
                            | "repo-map"
                    )
                ) =>
            {
                if i + 1 < args.len() {
                    match subcommand {
                        Some("install-agent") => {
                            agent_install_opts.repo = Some(args[i + 1].clone())
                        }
                        Some("uninstall-agent") => {
                            agent_uninstall_opts.repo = Some(args[i + 1].clone())
                        }
                        Some("search") => search_opts.repo = Some(args[i + 1].clone()),
                        Some("investigate") => investigate_opts.repo = Some(args[i + 1].clone()),
                        Some("ask") => ask_opts.repo = Some(args[i + 1].clone()),
                        Some("hydrate") => hydrate_opts.repo = Some(args[i + 1].clone()),
                        Some("repo-map") => repo_map_opts.repo = Some(args[i + 1].clone()),
                        _ => {}
                    }
                    i += 1;
                }
            }
            "--target" if matches!(subcommand, Some("install-agent" | "uninstall-agent")) => {
                if i + 1 < args.len() {
                    match subcommand {
                        Some("install-agent") => {
                            agent_install_opts.targets = parse_id_list(&args[i + 1])
                        }
                        Some("uninstall-agent") => {
                            agent_uninstall_opts.targets = parse_id_list(&args[i + 1])
                        }
                        _ => {}
                    }
                    i += 1;
                }
            }
            "--scope" if matches!(subcommand, Some("install-agent" | "uninstall-agent")) => {
                if i + 1 < args.len() {
                    match subcommand {
                        Some("install-agent") => agent_install_opts.scope = args[i + 1].clone(),
                        Some("uninstall-agent") => agent_uninstall_opts.scope = args[i + 1].clone(),
                        _ => {}
                    }
                    i += 1;
                }
            }
            "--print-config" if matches!(subcommand, Some("install-agent")) => {
                agent_install_opts.print_config = true;
            }
            "--yes" if matches!(subcommand, Some("install-agent" | "uninstall-agent")) => {
                match subcommand {
                    Some("install-agent") => agent_install_opts.yes = true,
                    Some("uninstall-agent") => agent_uninstall_opts.yes = true,
                    _ => {}
                }
            }
            "--no-instructions"
                if matches!(subcommand, Some("install-agent" | "uninstall-agent")) =>
            {
                match subcommand {
                    Some("install-agent") => agent_install_opts.no_instructions = true,
                    Some("uninstall-agent") => agent_uninstall_opts.no_instructions = true,
                    _ => {}
                }
            }
            "--no-mcp" if matches!(subcommand, Some("install-agent" | "uninstall-agent")) => {
                match subcommand {
                    Some("install-agent") => agent_install_opts.no_mcp = true,
                    Some("uninstall-agent") => agent_uninstall_opts.no_mcp = true,
                    _ => {}
                }
            }
            "--limit" if matches!(subcommand, Some("search")) => {
                if i + 1 < args.len() {
                    if let Ok(limit) = args[i + 1].parse::<u32>() {
                        search_opts.limit = Some(limit);
                    }
                    i += 1;
                }
            }
            "--context" if matches!(subcommand, Some("search")) => {
                if i + 1 < args.len() {
                    search_opts.context = Some(args[i + 1].clone());
                    i += 1;
                }
            }
            "--mode" if matches!(subcommand, Some("investigate" | "ask" | "hydrate")) => {
                if i + 1 < args.len() {
                    match subcommand {
                        Some("investigate") => investigate_opts.mode = Some(args[i + 1].clone()),
                        Some("ask") => ask_opts.mode = Some(args[i + 1].clone()),
                        Some("hydrate") => hydrate_opts.mode = Some(args[i + 1].clone()),
                        _ => {}
                    }
                    i += 1;
                }
            }
            "--target" if matches!(subcommand, Some("investigate" | "ask")) => {
                if i + 1 < args.len() {
                    match subcommand {
                        Some("investigate") => investigate_opts.target = Some(args[i + 1].clone()),
                        Some("ask") => ask_opts.target = Some(args[i + 1].clone()),
                        _ => {}
                    }
                    i += 1;
                }
            }
            "--file-path" if matches!(subcommand, Some("investigate" | "ask")) => {
                if i + 1 < args.len() {
                    match subcommand {
                        Some("investigate") => {
                            investigate_opts.file_path = Some(args[i + 1].clone())
                        }
                        Some("ask") => ask_opts.file_path = Some(args[i + 1].clone()),
                        _ => {}
                    }
                    i += 1;
                }
            }
            "--max-hops" if matches!(subcommand, Some("investigate")) => {
                if i + 1 < args.len() {
                    if let Ok(max_hops) = args[i + 1].parse::<u32>() {
                        investigate_opts.max_hops = Some(max_hops);
                    }
                    i += 1;
                }
            }
            "--max-evidence" if matches!(subcommand, Some("ask")) => {
                if i + 1 < args.len() {
                    if let Ok(max_evidence) = args[i + 1].parse::<u32>() {
                        ask_opts.max_evidence = Some(max_evidence);
                    }
                    i += 1;
                }
            }
            "--quality" if matches!(subcommand, Some("ask")) => {
                if i + 1 < args.len() {
                    ask_opts.quality = Some(args[i + 1].clone());
                    i += 1;
                }
            }
            "--ids" if matches!(subcommand, Some("hydrate")) => {
                if i + 1 < args.len() {
                    hydrate_opts.ids = parse_id_list(&args[i + 1]);
                    i += 1;
                }
            }
            "--verbose" if matches!(subcommand, Some("hydrate")) => {
                hydrate_opts.verbose = true;
            }
            "--timeout"
                if matches!(
                    subcommand,
                    Some("search" | "investigate" | "ask" | "hydrate" | "repo-map")
                ) =>
            {
                if i + 1 < args.len() {
                    match subcommand {
                        Some("search") => search_opts.timeout = Some(args[i + 1].clone()),
                        Some("investigate") => investigate_opts.timeout = Some(args[i + 1].clone()),
                        Some("ask") => ask_opts.timeout = Some(args[i + 1].clone()),
                        Some("hydrate") => hydrate_opts.timeout = Some(args[i + 1].clone()),
                        Some("repo-map") => repo_map_opts.timeout = Some(args[i + 1].clone()),
                        _ => {}
                    }
                    i += 1;
                }
            }
            "--no-start"
                if matches!(
                    subcommand,
                    Some("search" | "investigate" | "ask" | "hydrate" | "repo-map")
                ) =>
            {
                match subcommand {
                    Some("search") => search_opts.no_start = true,
                    Some("investigate") => investigate_opts.no_start = true,
                    Some("ask") => ask_opts.no_start = true,
                    Some("hydrate") => hydrate_opts.no_start = true,
                    Some("repo-map") => repo_map_opts.no_start = true,
                    _ => {}
                }
            }
            "--budget" if matches!(subcommand, Some("repo-map")) => {
                if i + 1 < args.len() {
                    if let Ok(budget) = args[i + 1].parse::<u32>() {
                        repo_map_opts.budget = Some(budget);
                    }
                    i += 1;
                }
            }
            "--max-files" if matches!(subcommand, Some("repo-map")) => {
                if i + 1 < args.len() {
                    if let Ok(max_files) = args[i + 1].parse::<u32>() {
                        repo_map_opts.max_files = Some(max_files);
                    }
                    i += 1;
                }
            }
            "--max-symbols-per-file" if matches!(subcommand, Some("repo-map")) => {
                if i + 1 < args.len() {
                    if let Ok(max_symbols_per_file) = args[i + 1].parse::<u32>() {
                        repo_map_opts.max_symbols_per_file = Some(max_symbols_per_file);
                    }
                    i += 1;
                }
            }
            "--json"
                if matches!(
                    subcommand,
                    Some("search" | "investigate" | "ask" | "hydrate" | "repo-map")
                ) =>
            {
                match subcommand {
                    Some("search") => search_opts.json = true,
                    Some("investigate") => investigate_opts.json = true,
                    Some("ask") => ask_opts.json = true,
                    Some("hydrate") => hydrate_opts.json = true,
                    Some("repo-map") => repo_map_opts.json = true,
                    _ => {}
                }
            }
            "--pretty"
                if matches!(
                    subcommand,
                    Some("search" | "investigate" | "ask" | "hydrate" | "repo-map")
                ) =>
            {
                match subcommand {
                    Some("search") => search_opts.pretty = true,
                    Some("investigate") => investigate_opts.pretty = true,
                    Some("ask") => ask_opts.pretty = true,
                    Some("hydrate") => hydrate_opts.pretty = true,
                    Some("repo-map") => repo_map_opts.pretty = true,
                    _ => {}
                }
            }
            _ if matches!(subcommand, Some("search" | "investigate" | "ask")) => {
                query_words.push(arg.to_string());
            }
            _ => {}
        }
        i += 1;
    }

    if matches!(subcommand, Some("search")) {
        search_opts.query = query_words.join(" ");
    }
    if matches!(subcommand, Some("investigate")) {
        investigate_opts.question = query_words.join(" ");
    }
    if matches!(subcommand, Some("ask")) {
        ask_opts.question = query_words.join(" ");
    }

    cli.command = match subcommand {
        Some("install") => Command::Install(install_opts),
        Some("uninstall") => Command::Uninstall,
        Some("start") => Command::Start,
        Some("stop") => Command::Stop,
        Some("status") => Command::Status,
        Some("migrate") => Command::Migrate(migrate_opts),
        Some("install-agent") => Command::InstallAgent(agent_install_opts),
        Some("uninstall-agent") => Command::UninstallAgent(agent_uninstall_opts),
        Some("search") => Command::Search(search_opts),
        Some("investigate") => Command::Investigate(investigate_opts),
        Some("ask") => Command::Ask(ask_opts),
        Some("hydrate") => Command::Hydrate(hydrate_opts),
        Some("repo-map") => Command::RepoMap(repo_map_opts),
        _ => Command::Run,
    };

    cli
}

fn parse_id_list(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string)
        .collect()
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
    println!("  code-intelligence-mcp-server install-agent [opts] Install managed agent guidance");
    println!("  code-intelligence-mcp-server uninstall-agent [opts] Remove managed agent guidance");
    println!(
        "  code-intelligence-mcp-server search [opts] QUERY  Search indexed code via the daemon"
    );
    println!(
        "  code-intelligence-mcp-server investigate [opts] QUESTION  Run a multi-hop code investigation"
    );
    println!("  code-intelligence-mcp-server ask [opts] QUESTION  Retrieve grounded evidence for a question");
    println!(
        "  code-intelligence-mcp-server hydrate [opts] --ids IDS  Fetch source bodies for symbol IDs"
    );
    println!("  code-intelligence-mcp-server repo-map [opts]  Print a compact project map");
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
    println!("agent install flags:");
    println!("  --target LIST           auto, codex, claude, cursor, opencode, generic, or all");
    println!("  --scope SCOPE           project or user (default: project)");
    println!("  --repo PATH             Project root for instruction files");
    println!("  --port PORT             MCP endpoint port in generated snippets");
    println!("  --print-config          Print MCP config and instruction block without writing");
    println!("  --dry-run               Print planned writes without changing files");
    println!("  --yes                   Reserved for non-interactive installs");
    println!("  --no-instructions       Skip instruction file updates");
    println!("  --no-mcp                Skip MCP config/snippet output");
    println!();
    println!("agent query flags:");
    println!("  --repo PATH             Workspace root (default: current directory)");
    println!("  --json                  Print machine-readable JSON");
    println!("  --pretty                Pretty-print JSON");
    println!("  --timeout DURATION      Daemon request timeout, e.g. 500ms, 2s, 1m");
    println!("  --no-start              Fail if the daemon is not running");
    println!("  --limit N               search result limit");
    println!("  --context MODE          search context: none, snippets, or full");
    println!("  --mode MODE             investigate mode: auto, discover, trace, data, impact, dependency, module");
    println!("  --target SYMBOL         investigate pivot symbol");
    println!("  --file-path PATH        investigate file disambiguation");
    println!("  --max-hops N            investigate hop limit");
    println!("  --max-evidence N        ask evidence limit");
    println!("  --quality MODE          ask quality: fast or balanced");
    println!("  --ids IDS               hydrate symbol IDs (comma-separated)");
    println!("  --verbose               hydrate includes context item metadata");
    println!("  --budget N              repo-map approximate token budget");
    println!("  --max-files N           repo-map file cap");
    println!("  --max-symbols-per-file N  repo-map symbol cap per file");
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
    fn parse_args_recognises_install_agent_defaults() {
        let cli = parse_args(&["bin".into(), "install-agent".into()]);
        match cli.command {
            Command::InstallAgent(opts) => {
                assert_eq!(opts.targets, vec!["auto".to_string()]);
                assert_eq!(opts.scope, "project");
                assert_eq!(opts.repo, None);
                assert_eq!(opts.port, None);
                assert!(!opts.print_config);
                assert!(!opts.dry_run);
                assert!(!opts.no_instructions);
                assert!(!opts.no_mcp);
            }
            _ => panic!("expected install-agent command, got {:?}", cli.command),
        }
    }

    #[test]
    fn parse_args_recognises_install_agent_flags() {
        let cli = parse_args(&[
            "bin".into(),
            "install-agent".into(),
            "--target".into(),
            "codex,claude".into(),
            "--scope".into(),
            "user".into(),
            "--repo".into(),
            "/tmp/project".into(),
            "--port".into(),
            "20000".into(),
            "--print-config".into(),
            "--dry-run".into(),
            "--yes".into(),
            "--no-instructions".into(),
            "--no-mcp".into(),
        ]);
        match cli.command {
            Command::InstallAgent(opts) => {
                assert_eq!(
                    opts.targets,
                    vec!["codex".to_string(), "claude".to_string()]
                );
                assert_eq!(opts.scope, "user");
                assert_eq!(opts.repo.as_deref(), Some("/tmp/project"));
                assert_eq!(opts.port, Some(20000));
                assert!(opts.print_config);
                assert!(opts.dry_run);
                assert!(opts.yes);
                assert!(opts.no_instructions);
                assert!(opts.no_mcp);
            }
            _ => panic!("expected install-agent command, got {:?}", cli.command),
        }
    }

    #[test]
    fn parse_args_recognises_uninstall_agent_flags() {
        let cli = parse_args(&[
            "bin".into(),
            "uninstall-agent".into(),
            "--target".into(),
            "cursor".into(),
            "--scope".into(),
            "project".into(),
            "--repo".into(),
            "/tmp/project".into(),
            "--dry-run".into(),
            "--yes".into(),
            "--no-mcp".into(),
        ]);
        match cli.command {
            Command::UninstallAgent(opts) => {
                assert_eq!(opts.targets, vec!["cursor".to_string()]);
                assert_eq!(opts.scope, "project");
                assert_eq!(opts.repo.as_deref(), Some("/tmp/project"));
                assert!(opts.dry_run);
                assert!(opts.yes);
                assert!(!opts.no_instructions);
                assert!(opts.no_mcp);
            }
            _ => panic!("expected uninstall-agent command, got {:?}", cli.command),
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

    #[test]
    fn parse_args_recognises_search_subcommand() {
        let cli = parse_args(&[
            "bin".into(),
            "search".into(),
            "--repo".into(),
            ".".into(),
            "--limit".into(),
            "7".into(),
            "--context".into(),
            "snippets".into(),
            "--json".into(),
            "FastAPI".into(),
            "auth".into(),
        ]);
        match cli.command {
            Command::Search(opts) => {
                assert_eq!(opts.repo.as_deref(), Some("."));
                assert_eq!(opts.limit, Some(7));
                assert_eq!(opts.context.as_deref(), Some("snippets"));
                assert!(opts.json);
                assert_eq!(opts.query, "FastAPI auth");
            }
            _ => panic!("expected search command, got {:?}", cli.command),
        }
    }

    #[test]
    fn parse_args_recognises_investigate_subcommand() {
        let cli = parse_args(&[
            "bin".into(),
            "investigate".into(),
            "--repo".into(),
            "/tmp/project".into(),
            "--mode".into(),
            "impact".into(),
            "--target".into(),
            "authenticate_request".into(),
            "--max-hops".into(),
            "4".into(),
            "--json".into(),
            "what".into(),
            "breaks".into(),
        ]);
        match cli.command {
            Command::Investigate(opts) => {
                assert_eq!(opts.repo.as_deref(), Some("/tmp/project"));
                assert_eq!(opts.mode.as_deref(), Some("impact"));
                assert_eq!(opts.target.as_deref(), Some("authenticate_request"));
                assert_eq!(opts.max_hops, Some(4));
                assert!(opts.json);
                assert_eq!(opts.question, "what breaks");
            }
            _ => panic!("expected investigate command, got {:?}", cli.command),
        }
    }

    #[test]
    fn parse_args_recognises_ask_subcommand() {
        let cli = parse_args(&[
            "bin".into(),
            "ask".into(),
            "--repo".into(),
            ".".into(),
            "--mode".into(),
            "discover".into(),
            "--target".into(),
            "parse_args".into(),
            "--max-evidence".into(),
            "6".into(),
            "--quality".into(),
            "fast".into(),
            "--json".into(),
            "where".into(),
            "is".into(),
            "parse_args".into(),
        ]);
        match cli.command {
            Command::Ask(opts) => {
                assert_eq!(opts.repo.as_deref(), Some("."));
                assert_eq!(opts.mode.as_deref(), Some("discover"));
                assert_eq!(opts.target.as_deref(), Some("parse_args"));
                assert_eq!(opts.max_evidence, Some(6));
                assert_eq!(opts.quality.as_deref(), Some("fast"));
                assert!(opts.json);
                assert_eq!(opts.question, "where is parse_args");
            }
            _ => panic!("expected ask command, got {:?}", cli.command),
        }
    }

    #[test]
    fn parse_args_recognises_hydrate_subcommand() {
        let cli = parse_args(&[
            "bin".into(),
            "hydrate".into(),
            "--repo".into(),
            "/tmp/project".into(),
            "--ids".into(),
            "sym_a,sym_b".into(),
            "--mode".into(),
            "full".into(),
            "--verbose".into(),
            "--json".into(),
        ]);
        match cli.command {
            Command::Hydrate(opts) => {
                assert_eq!(opts.repo.as_deref(), Some("/tmp/project"));
                assert_eq!(opts.ids, vec!["sym_a".to_string(), "sym_b".to_string()]);
                assert_eq!(opts.mode.as_deref(), Some("full"));
                assert!(opts.verbose);
                assert!(opts.json);
            }
            _ => panic!("expected hydrate command, got {:?}", cli.command),
        }
    }

    #[test]
    fn parse_args_recognises_repo_map_subcommand() {
        let cli = parse_args(&[
            "bin".into(),
            "repo-map".into(),
            "--repo".into(),
            ".".into(),
            "--budget".into(),
            "2500".into(),
            "--max-files".into(),
            "12".into(),
            "--max-symbols-per-file".into(),
            "4".into(),
            "--json".into(),
        ]);
        match cli.command {
            Command::RepoMap(opts) => {
                assert_eq!(opts.repo.as_deref(), Some("."));
                assert_eq!(opts.budget, Some(2500));
                assert_eq!(opts.max_files, Some(12));
                assert_eq!(opts.max_symbols_per_file, Some(4));
                assert!(opts.json);
            }
            _ => panic!("expected repo-map command, got {:?}", cli.command),
        }
    }

    #[test]
    fn parse_args_recognises_common_agent_query_controls() {
        let cli = parse_args(&[
            "bin".into(),
            "search".into(),
            "--repo".into(),
            ".".into(),
            "--timeout".into(),
            "2s".into(),
            "--no-start".into(),
            "--json".into(),
            "auth".into(),
        ]);
        match cli.command {
            Command::Search(opts) => {
                assert_eq!(opts.timeout.as_deref(), Some("2s"));
                assert!(opts.no_start);
                assert!(opts.json);
            }
            _ => panic!("expected search command, got {:?}", cli.command),
        }
    }
}
