//! CLI argument parsing and help text.
//!
//! Since v4.0 the server has one mode (HTTP daemon). The legacy `--standalone`
//! flag and `CIMCP_MODE` env var are still accepted to keep old configs from
//! erroring out, but they no longer change behaviour.

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CliArgs {
    pub help: bool,
    pub version: bool,
    pub host: Option<String>,
    pub port: Option<u16>,
    pub discovery_port: Option<u16>,
}

pub fn parse_args(args: &[String]) -> CliArgs {
    let mut cli = CliArgs::default();

    let mut i = 1; // Skip program name at index 0
    while i < args.len() {
        let arg = &args[i];
        match arg.as_str() {
            "-h" | "--help" | "help" => cli.help = true,
            "-V" | "--version" | "version" => cli.version = true,
            // Accepted for backward compatibility; no-op in v4.
            "--standalone" => {}
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
            _ => {}
        }
        i += 1;
    }

    cli
}

pub fn print_help() {
    println!("code-intelligence-mcp-server");
    println!();
    println!("HTTP MCP daemon for local code intelligence (index + search + context).");
    println!();
    println!("Usage:");
    println!("  code-intelligence-mcp-server");
    println!("  code-intelligence-mcp-server --host 0.0.0.0 --port 4444");
    println!();
    println!("Flags:");
    println!("  -h, --help              Show this help");
    println!("  -V, --version           Show version");
    println!("  --host HOST             Override listen address (default: 127.0.0.1)");
    println!("  --port PORT             Override listen port (default: 17800)");
    println!("  --discovery-port PORT   Discovery endpoint port (default: MCP port + 1)");
    println!();
    println!("Configuration file: ~/.code-intelligence/server.toml");
    println!("Per-repo data stored in: ~/.code-intelligence/repos/<repo-id>/");
    println!();
    println!("Common env (defaults shown):");
    println!("  EMBEDDINGS_MODEL_DIR=<auto>           (default: ~/.code-intelligence/models/jina-code-embeddings-1.5b-gguf)");
    println!("  EMBEDDINGS_BACKEND=llamacpp|hash      (default: llamacpp)");
    println!("  EMBEDDINGS_DEVICE=cpu|metal           (default: metal)");
    println!("  EMBEDDING_BATCH_SIZE=32");
    println!("  MAX_CONTEXT_BYTES=200000");
    println!("  WATCH_MODE=true|false                 (default: true)");
    println!();
    println!("LLM description generation:");
    println!("  LLM_ENABLED=true|false                (default: true)");
    println!("  LLM_DEVICE=cpu|metal                  (default: cpu)");
    println!("  LLM_MODEL_DIR=<auto>                  (default: ~/.code-intelligence/models/qwen2.5-coder-1.5b-gguf)");
    println!("  LLM_MAX_TOKENS=50                     (default: 50)");
    println!("  LLM_BATCH_COMMIT=10                   (default: 10)");
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
        assert!(parse_args(&["bin".into(), "version".into()]).version);
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
    fn parse_args_handles_missing_values() {
        assert!(parse_args(&["bin".into(), "--host".into()]).host.is_none());
        assert!(parse_args(&["bin".into(), "--port".into()]).port.is_none());
    }

    #[test]
    fn parse_args_defaults_to_no_overrides() {
        let cli = parse_args(&["bin".into()]);
        assert!(!cli.help);
        assert!(!cli.version);
        assert!(cli.host.is_none());
        assert!(cli.port.is_none());
        assert!(cli.discovery_port.is_none());
    }

    #[test]
    fn parse_args_ignores_unknown_args() {
        let cli = parse_args(&["bin".into(), "--unknown".into()]);
        assert!(!cli.help);
        assert!(!cli.version);
    }

    #[test]
    fn parse_args_silently_accepts_legacy_standalone_flag() {
        // v3 configs passed --standalone; v4 still accepts it as a no-op.
        let cli = parse_args(&["bin".into(), "--standalone".into()]);
        assert!(!cli.help);
        assert!(!cli.version);
    }

    #[test]
    fn parse_args_parses_discovery_port() {
        let cli = parse_args(&["bin".into(), "--discovery-port".into(), "5000".into()]);
        assert_eq!(cli.discovery_port, Some(5000));
    }
}
