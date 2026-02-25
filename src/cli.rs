//! CLI argument parsing and help text

use std::env;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliArgs {
    pub help: bool,
    pub version: bool,
    pub standalone: bool,
    pub host: Option<String>,
    pub port: Option<u16>,
    pub chat: bool,
    pub chat_port: Option<u16>,
}

pub fn parse_args(args: &[String]) -> CliArgs {
    let mut cli = CliArgs {
        help: false,
        version: false,
        standalone: false,
        host: None,
        port: None,
        chat: false,
        chat_port: None,
    };

    // Check for standalone mode via env var
    if env::var("CIMCP_MODE")
        .ok()
        .map(|v| v.to_lowercase() == "standalone")
        .unwrap_or(false)
    {
        cli.standalone = true;
    }

    // Check for chat mode via env var
    if env::var("CIMCP_CHAT")
        .ok()
        .map(|v| matches!(v.trim().to_lowercase().as_str(), "1" | "true" | "yes"))
        .unwrap_or(false)
    {
        cli.chat = true;
    }

    let mut i = 1; // Skip program name at index 0
    while i < args.len() {
        let arg = &args[i];
        match arg.as_str() {
            "-h" | "--help" | "help" => cli.help = true,
            "-V" | "--version" | "version" => cli.version = true,
            "--standalone" => cli.standalone = true,
            "--host" => {
                if i + 1 < args.len() {
                    cli.host = Some(args[i + 1].clone());
                    i += 1; // Skip the value
                }
            }
            "--port" => {
                if i + 1 < args.len() {
                    if let Ok(port) = args[i + 1].parse::<u16>() {
                        cli.port = Some(port);
                    }
                    i += 1; // Skip the value
                }
            }
            "--chat" => cli.chat = true,
            "--chat-port" => {
                if i + 1 < args.len() {
                    if let Ok(port) = args[i + 1].parse::<u16>() {
                        cli.chat_port = Some(port);
                    }
                    i += 1;
                }
            }
            _ => {} // Ignore unknown args
        }
        i += 1;
    }

    cli
}

pub fn print_help() {
    println!("code-intelligence-mcp-server");
    println!();
    println!("MCP server for local code intelligence (index + search + context).");
    println!();
    println!("Usage:");
    println!("  Embedded mode (stdio, single repo):");
    println!("    code-intelligence-mcp-server");
    println!("    BASE_DIR=/path/to/repo code-intelligence-mcp-server");
    println!();
    println!("  Standalone mode (HTTP, multi-repo):");
    println!("    code-intelligence-mcp-server --standalone");
    println!("    code-intelligence-mcp-server --standalone --host 0.0.0.0 --port 4444");
    println!("    CIMCP_MODE=standalone code-intelligence-mcp-server");
    println!();
    println!("Flags:");
    println!("  -h, --help              Show this help");
    println!("  -V, --version           Show version");
    println!("  --standalone            Run in standalone HTTP mode");
    println!("  --host HOST             Override listen address (standalone only)");
    println!("  --port PORT             Override listen port (standalone only)");
    println!("  --chat                  Enable chat UI (standalone only)");
    println!("  --chat-port PORT        Chat UI port (default: 3334)");
    println!();
    println!("Embedded mode (default):");
    println!("  Required env:");
    println!("    BASE_DIR=/absolute/path/to/repo");
    println!();
    println!("  Common env (defaults shown):");
    println!("    EMBEDDINGS_MODEL_DIR=/path/to/cache   (default: ~/.code-intelligence/models/jina-code-embeddings-1.5b-gguf)");
    println!("    EMBEDDINGS_BACKEND=llamacpp|hash      (default: llamacpp)");
    println!("    EMBEDDINGS_DEVICE=cpu|metal           (default: metal)");
    println!("    EMBEDDING_BATCH_SIZE=32");
    println!("    DB_PATH=<auto>                       (per-repo: ~/.code-intelligence/repos/<hash>/code-intelligence.db)");
    println!("    VECTOR_DB_PATH=<auto>                (per-repo: ~/.code-intelligence/repos/<hash>/vectors)");
    println!("    TANTIVY_INDEX_PATH=<auto>            (per-repo: ~/.code-intelligence/repos/<hash>/tantivy-index)");
    println!("    MAX_CONTEXT_BYTES=200000");
    println!("    WATCH_MODE=true|false                (default: true)");
    println!("    REPO_ROOTS=/path/a,/path/b           (default: BASE_DIR only)");
    println!();
    println!("  LLM description generation:");
    println!("    LLM_ENABLED=true|false               (default: true)");
    println!("    LLM_DEVICE=cpu|metal                  (default: cpu)");
    println!("    LLM_MODEL_DIR=/path/to/model          (default: ~/.code-intelligence/models/qwen2.5-coder-1.5b-gguf)");
    println!("    LLM_MAX_TOKENS=30                     (default: 30)");
    println!("    LLM_BATCH_COMMIT=10                   (default: 10)");
    println!();
    println!("Standalone mode:");
    println!("  Configuration file: ~/.code-intelligence/server.toml");
    println!("  Default host: 127.0.0.1");
    println!("  Default port: 3333");
    println!("  Per-repo data stored in: ~/.code-intelligence/repos/<repo-id>/");
    println!();
    println!("Chat mode (requires --standalone --chat):");
    println!("  Starts a ChatGPT-style web UI for codebase Q&A.");
    println!("  Downloads Qwen2.5-Coder-14B (~9GB) on first launch.");
    println!("  Default chat port: 3334");
    println!("  Env vars: CIMCP_CHAT=true, CIMCP_CHAT_PORT=4000");
    println!();
    println!("Tools:");
    println!("  search_code, refresh_index, get_definition, find_references, get_file_symbols,");
    println!("  get_call_hierarchy, get_type_graph, get_usage_examples, get_index_stats, get_similarity_cluster");
}

pub fn print_version() {
    println!("{}", env!("CARGO_PKG_VERSION"));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_args_detects_help_flags() {
        let cli = parse_args(&["bin".to_string(), "--help".to_string()]);
        assert!(cli.help);

        let cli = parse_args(&["bin".to_string(), "-h".to_string()]);
        assert!(cli.help);

        let cli = parse_args(&["bin".to_string(), "help".to_string()]);
        assert!(cli.help);
    }

    #[test]
    fn parse_args_detects_version_flags() {
        let cli = parse_args(&["bin".to_string(), "--version".to_string()]);
        assert!(cli.version);

        let cli = parse_args(&["bin".to_string(), "-V".to_string()]);
        assert!(cli.version);

        let cli = parse_args(&["bin".to_string(), "version".to_string()]);
        assert!(cli.version);
    }

    #[test]
    fn parse_args_detects_standalone_flag() {
        let cli = parse_args(&["bin".to_string(), "--standalone".to_string()]);
        assert!(cli.standalone);
    }

    #[test]
    fn parse_args_parses_host_and_port() {
        let cli = parse_args(&[
            "bin".to_string(),
            "--host".to_string(),
            "0.0.0.0".to_string(),
            "--port".to_string(),
            "4444".to_string(),
        ]);
        assert_eq!(cli.host, Some("0.0.0.0".to_string()));
        assert_eq!(cli.port, Some(4444));
    }

    #[test]
    fn parse_args_handles_missing_values() {
        let cli = parse_args(&["bin".to_string(), "--host".to_string()]);
        assert!(cli.host.is_none());

        let cli = parse_args(&["bin".to_string(), "--port".to_string()]);
        assert!(cli.port.is_none());
    }

    #[test]
    fn parse_args_defaults_to_embedded_mode() {
        let cli = parse_args(&["bin".to_string()]);
        assert!(!cli.help);
        assert!(!cli.version);
        assert!(!cli.standalone);
        assert!(cli.host.is_none());
        assert!(cli.port.is_none());
        assert!(!cli.chat);
        assert!(cli.chat_port.is_none());
    }

    #[test]
    fn parse_args_ignores_unknown_args() {
        let cli = parse_args(&["bin".to_string(), "--unknown".to_string()]);
        assert!(!cli.help);
        assert!(!cli.version);
        assert!(!cli.standalone);
    }

    #[test]
    fn parse_args_detects_chat_flag() {
        let cli = parse_args(&["bin".to_string(), "--chat".to_string()]);
        assert!(cli.chat);
        assert!(cli.chat_port.is_none());
    }

    #[test]
    fn parse_args_parses_chat_port() {
        let cli = parse_args(&[
            "bin".to_string(),
            "--chat".to_string(),
            "--chat-port".to_string(),
            "4000".to_string(),
        ]);
        assert!(cli.chat);
        assert_eq!(cli.chat_port, Some(4000));
    }
}
