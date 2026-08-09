//! Code Intelligence MCP Server - Main entry point

#![allow(unexpected_cfgs)]

use rust_mcp_sdk::{
    error::{McpSdkError, SdkResult},
    schema::{
        Implementation, InitializeResult, ProtocolVersion, ServerCapabilities,
        ServerCapabilitiesTools,
    },
};
use std::sync::Arc;
use tracing::{debug, error, info};
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter, Layer};

use code_intelligence_mcp_server::cli;
use code_intelligence_mcp_server::embeddings::{
    create_embedder, default_embedding_dim, ensure_native_llama_available, DeferredEmbedder,
    Embedder, SharedEmbedder, TruncatingEmbedder,
};
use code_intelligence_mcp_server::path::{Utf8Path, Utf8PathBuf};
use code_intelligence_mcp_server::reranker::{
    create_reranker, deferred::DeferredReranker, Reranker,
};

/// top-k passed to both the deferred wrapper and the real reranker. Matches the
/// per-repo `Config::reranker_top_k` default in `StandaloneConfig::repo_config`.
const RERANKER_TOP_K: usize = 20;

/// Slot a background task fills once the real reranker model is loaded.
type RerankerSlot = std::sync::Arc<std::sync::Mutex<Option<std::sync::Arc<dyn Reranker>>>>;

/// Type alias for the (embedder, optional-deferred-slot) pair returned by embedder creation.
type EmbedderWithSlot = (
    std::sync::Arc<SharedEmbedder>,
    Option<std::sync::Arc<std::sync::Mutex<Option<Box<dyn Embedder>>>>>,
);

/// Optionally wrap an embedder with Matryoshka truncation.
fn maybe_truncate(
    embedder: Box<dyn Embedder>,
    truncate_dim: Option<usize>,
) -> anyhow::Result<Box<dyn Embedder>> {
    match truncate_dim {
        Some(dim) => Ok(Box::new(TruncatingEmbedder::new(embedder, dim)?)),
        None => Ok(embedder),
    }
}

#[tokio::main]
async fn main() -> SdkResult<()> {
    let args = std::env::args().collect::<Vec<_>>();
    let cli_args = cli::parse_args(&args);

    if cli_args.help {
        cli::print_help();
        return Ok(());
    }
    if cli_args.version {
        cli::print_version();
        return Ok(());
    }

    // Lifecycle subcommands run synchronously without booting the daemon.
    if !matches!(
        cli_args.command,
        code_intelligence_mcp_server::cli::Command::Run
    ) {
        match dispatch_subcommand(cli_args.command, cli_args.port).await {
            Ok(()) => return Ok(()),
            Err(err) => {
                if let Some(failure) = err.downcast_ref::<CliFailure>() {
                    failure.emit();
                    std::process::exit(failure.exit_code());
                }
                return Err(McpSdkError::Internal {
                    description: err.to_string(),
                });
            }
        }
    }

    // Set up file logging to global ~/.code-intelligence/logs directory
    let global_dir = code_intelligence_mcp_server::config::get_data_dir();
    let logs_dir = global_dir.join("logs");

    // Create logs directory if it doesn't exist
    std::fs::create_dir_all(&logs_dir).map_err(|err| McpSdkError::Internal {
        description: format!("Failed to create logs directory: {}", err),
    })?;

    // Clean up log files older than 7 days
    code_intelligence_mcp_server::logging::cleanup_old_logs(&logs_dir, 7);

    // Create a daily rotating file appender for global server log
    let file_appender = tracing_appender::rolling::daily(&logs_dir, "server.log");
    let (non_blocking_file, _guard) = tracing_appender::non_blocking(file_appender);

    // Create a daily rotating file appender for MCP access log
    let access_appender = tracing_appender::rolling::daily(&logs_dir, "access.log");
    let (non_blocking_access, _access_guard) = tracing_appender::non_blocking(access_appender);

    // Set up layered subscriber with stderr, file, access log, and in-process
    // broadcast for the SSE /api/logs/stream endpoint.
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let log_broadcaster = code_intelligence_mcp_server::log_broadcast::LogBroadcaster::new();

    tracing_subscriber::registry()
        .with(env_filter)
        .with(fmt::layer().with_writer(std::io::stderr).with_ansi(true))
        .with(fmt::layer().with_writer(non_blocking_file).with_ansi(false))
        .with(
            fmt::layer()
                .with_writer(non_blocking_access)
                .with_ansi(false)
                .with_filter(
                    tracing_subscriber::filter::Targets::new()
                        .with_target("mcp_access", tracing::Level::INFO),
                ),
        )
        .with(
            fmt::layer()
                .with_writer(log_broadcaster.make_writer())
                .with_ansi(false),
        )
        .init();

    // Keep guards alive for the duration of the program
    std::mem::forget(_guard);
    std::mem::forget(_access_guard);

    info!(
        version = env!("CARGO_PKG_VERSION"),
        logs_dir = %logs_dir,
        "Starting code-intelligence-mcp-server"
    );

    // Raise the soft FD limit toward the hard limit so parallel indexing
    // (Tantivy mmap, LanceDB fragments, pooled SQLite WAL connections, plus
    // the embedding/reranker/LLM models) doesn't trip the macOS default of
    // 256 open files.
    match code_intelligence_mcp_server::os::raise_fd_limit_to_hard() {
        Ok((old_soft, new_soft, hard)) if new_soft > old_soft => {
            info!(old_soft, new_soft, hard, "Raised RLIMIT_NOFILE soft limit");
        }
        Ok((soft, _, hard)) => {
            debug!(soft, hard, "RLIMIT_NOFILE already at or above hard limit");
        }
        Err(err) => {
            tracing::warn!(error = %err, "Failed to raise RLIMIT_NOFILE; indexing may hit FD limits under load");
        }
    }

    run_standalone(
        cli_args.host.as_deref(),
        cli_args.port,
        cli_args.discovery_port,
        log_broadcaster,
    )
    .await
}

async fn dispatch_subcommand(
    cmd: code_intelligence_mcp_server::cli::Command,
    port_override: Option<u16>,
) -> anyhow::Result<()> {
    use code_intelligence_mcp_server::agent_install;
    use code_intelligence_mcp_server::cli::Command;
    use code_intelligence_mcp_server::install;
    match cmd {
        Command::Run => unreachable!("dispatch_subcommand should not see Run"),
        Command::Install(opts) => install::handle_install(opts),
        Command::Uninstall => install::handle_uninstall(),
        Command::Start => install::handle_start(),
        Command::Stop => install::handle_stop(),
        Command::Status => install::handle_status(),
        Command::Migrate(opts) => install::handle_migrate(opts),
        Command::InstallAgent(opts) => agent_install::handle_install_agent(opts),
        Command::UninstallAgent(opts) => agent_install::handle_uninstall_agent(opts),
        Command::Search(opts) => handle_cli_search(opts, port_override).await,
        Command::Investigate(opts) => handle_cli_investigate(opts, port_override).await,
        Command::Ask(opts) => handle_cli_ask(opts, port_override).await,
        Command::Hydrate(opts) => handle_cli_hydrate(opts, port_override).await,
        Command::RepoMap(opts) => handle_cli_repo_map(opts, port_override).await,
        Command::Definition(opts) => {
            handle_cli_symbol_query("definition", "definition", opts, port_override).await
        }
        Command::References(opts) => {
            handle_cli_symbol_query("references", "references", opts, port_override).await
        }
        Command::CallHierarchy(opts) => {
            handle_cli_symbol_query("call-hierarchy", "call-hierarchy", opts, port_override).await
        }
        Command::TypeGraph(opts) => {
            handle_cli_symbol_query("type-graph", "type-graph", opts, port_override).await
        }
        Command::DependencyGraph(opts) => {
            handle_cli_symbol_query("dependency-graph", "dependency-graph", opts, port_override)
                .await
        }
        Command::Index(opts) => handle_cli_index(opts, port_override).await,
        Command::Capabilities(opts) => handle_cli_capabilities(opts),
    }
}

async fn handle_cli_search(
    opts: code_intelligence_mcp_server::cli::SearchOpts,
    port_override: Option<u16>,
) -> anyhow::Result<()> {
    if opts.query.trim().is_empty() {
        return Err(CliFailure::new(
            "search",
            CliErrorCode::InvalidArguments,
            "search query is required",
            opts.json,
            opts.pretty,
        )
        .into());
    }
    let runtime = CliQueryRuntime::new(
        "search",
        opts.timeout.clone(),
        opts.no_start,
        opts.json,
        opts.pretty,
    )?;
    let body = serde_json::json!({
        "repo": cli_repo(opts.repo)?,
        "query": opts.query,
        "limit": opts.limit,
        "context": opts.context,
        "exported_only": opts.exported_only,
    });
    let value = post_cli_query("search", body, port_override, &runtime).await?;
    fail_on_empty_results("search", &value, &runtime)?;
    print_cli_query_response(&value, opts.json, opts.pretty)
}

async fn handle_cli_investigate(
    opts: code_intelligence_mcp_server::cli::InvestigateOpts,
    port_override: Option<u16>,
) -> anyhow::Result<()> {
    if opts.question.trim().is_empty() {
        return Err(CliFailure::new(
            "investigate",
            CliErrorCode::InvalidArguments,
            "investigate question is required",
            opts.json,
            opts.pretty,
        )
        .into());
    }
    let runtime = CliQueryRuntime::new(
        "investigate",
        opts.timeout.clone(),
        opts.no_start,
        opts.json,
        opts.pretty,
    )?;
    let body = serde_json::json!({
        "repo": cli_repo(opts.repo)?,
        "question": opts.question,
        "target": opts.target,
        "file_path": opts.file_path,
        "mode": opts.mode,
        "max_hops": opts.max_hops,
    });
    let value = post_cli_query("investigate", body, port_override, &runtime).await?;
    print_cli_query_response(&value, opts.json, opts.pretty)
}

async fn handle_cli_ask(
    opts: code_intelligence_mcp_server::cli::AskOpts,
    port_override: Option<u16>,
) -> anyhow::Result<()> {
    if opts.question.trim().is_empty() {
        return Err(CliFailure::new(
            "ask",
            CliErrorCode::InvalidArguments,
            "ask question is required",
            opts.json,
            opts.pretty,
        )
        .into());
    }
    let runtime = CliQueryRuntime::new(
        "ask",
        opts.timeout.clone(),
        opts.no_start,
        opts.json,
        opts.pretty,
    )?;
    let body = serde_json::json!({
        "repo": cli_repo(opts.repo)?,
        "question": opts.question,
        "target": opts.target,
        "file_path": opts.file_path,
        "mode": opts.mode,
        "max_evidence": opts.max_evidence,
        "quality": opts.quality,
    });
    let value = post_cli_query("ask", body, port_override, &runtime).await?;
    print_cli_query_response(&value, opts.json, opts.pretty)
}

async fn handle_cli_hydrate(
    opts: code_intelligence_mcp_server::cli::HydrateOpts,
    port_override: Option<u16>,
) -> anyhow::Result<()> {
    if opts.ids.is_empty() {
        return Err(CliFailure::new(
            "hydrate",
            CliErrorCode::InvalidArguments,
            "hydrate requires --ids",
            opts.json,
            opts.pretty,
        )
        .into());
    }
    let runtime = CliQueryRuntime::new(
        "hydrate",
        opts.timeout.clone(),
        opts.no_start,
        opts.json,
        opts.pretty,
    )?;
    let body = serde_json::json!({
        "repo": cli_repo(opts.repo)?,
        "ids": opts.ids,
        "mode": opts.mode,
        "verbose": opts.verbose,
    });
    let value = post_cli_query("hydrate", body, port_override, &runtime).await?;
    fail_on_empty_results("hydrate", &value, &runtime)?;
    print_cli_query_response(&value, opts.json, opts.pretty)
}

async fn handle_cli_repo_map(
    opts: code_intelligence_mcp_server::cli::RepoMapOpts,
    port_override: Option<u16>,
) -> anyhow::Result<()> {
    let runtime = CliQueryRuntime::new(
        "repo-map",
        opts.timeout.clone(),
        opts.no_start,
        opts.json,
        opts.pretty,
    )?;
    let body = serde_json::json!({
        "repo": cli_repo(opts.repo)?,
        "budget_tokens": opts.budget,
        "max_files": opts.max_files,
        "max_symbols_per_file": opts.max_symbols_per_file,
    });
    let value = post_cli_query("repo-map", body, port_override, &runtime).await?;
    print_cli_query_response(&value, opts.json, opts.pretty)
}

async fn handle_cli_symbol_query(
    command: &'static str,
    endpoint: &str,
    opts: code_intelligence_mcp_server::cli::SymbolQueryOpts,
    port_override: Option<u16>,
) -> anyhow::Result<()> {
    if opts.symbol_name.trim().is_empty() {
        return Err(CliFailure::new(
            command,
            CliErrorCode::InvalidArguments,
            format!("{command} requires a symbol name"),
            opts.json,
            opts.pretty,
        )
        .into());
    }
    let runtime = CliQueryRuntime::new(
        command,
        opts.timeout.clone(),
        opts.no_start,
        opts.json,
        opts.pretty,
    )?;
    let body = serde_json::json!({
        "repo": cli_repo(opts.repo)?,
        "symbol_name": opts.symbol_name,
        "file": opts.file,
        "reference_type": opts.reference_type,
        "direction": opts.direction,
        "depth": opts.depth,
        "limit": opts.limit,
    });
    let value = post_cli_query(endpoint, body, port_override, &runtime).await?;
    fail_on_empty_results(command, &value, &runtime)?;
    print_cli_query_response(&value, opts.json, opts.pretty)
}

async fn handle_cli_index(
    opts: code_intelligence_mcp_server::cli::IndexOpts,
    port_override: Option<u16>,
) -> anyhow::Result<()> {
    let runtime = CliQueryRuntime::new(
        "index",
        opts.timeout.clone(),
        opts.no_start,
        opts.json,
        opts.pretty,
    )?;
    let action = opts.action.trim().to_ascii_lowercase();
    let value = match action.as_str() {
        "jobs" => get_cli_api("index", "/api/jobs", port_override, &runtime).await?,
        "status" | "approve" | "decline" | "refresh" => {
            let repo = cli_repo(opts.repo)?;
            let status_body = serde_json::json!({ "repo": repo.clone() });
            let status = post_cli_api(
                "index",
                "/api/index/status",
                status_body,
                port_override,
                &runtime,
            )
            .await?;
            match action.as_str() {
                "status" => status,
                "approve" | "decline" => {
                    post_cli_api(
                        "index",
                        "/api/consent",
                        serde_json::json!({ "repo": repo, "decision": action }),
                        port_override,
                        &runtime,
                    )
                    .await?
                }
                "refresh" => {
                    let repo_id = status
                        .get("repo_id")
                        .and_then(|value| value.as_str())
                        .ok_or_else(|| {
                            CliFailure::new(
                                "index",
                                CliErrorCode::Internal,
                                "index status response did not include repo_id",
                                opts.json,
                                opts.pretty,
                            )
                        })?;
                    post_cli_api(
                        "index",
                        &format!("/api/repos/{repo_id}/reindex"),
                        serde_json::json!({}),
                        port_override,
                        &runtime,
                    )
                    .await?
                }
                _ => unreachable!(),
            }
        }
        _ => {
            return Err(CliFailure::new(
                "index",
                CliErrorCode::InvalidArguments,
                format!(
                "unknown index action '{}'; expected status, approve, decline, refresh, or jobs",
                opts.action
            ),
                opts.json,
                opts.pretty,
            )
            .into())
        }
    };
    print_cli_query_response(&value, opts.json, opts.pretty)
}

fn handle_cli_capabilities(
    opts: code_intelligence_mcp_server::cli::OutputOpts,
) -> anyhow::Result<()> {
    print_cli_query_response(&cli_capabilities_value(), opts.json, opts.pretty)
}

fn cli_capabilities_value() -> serde_json::Value {
    let tools = code_intelligence_mcp_server::server::all_tools()
        .into_iter()
        .filter_map(|tool| serde_json::to_value(tool).ok())
        .filter_map(|mut tool| {
            let cli_command = match tool.get("name").and_then(serde_json::Value::as_str)? {
                "ask_code" => "ask",
                "investigate" => "investigate",
                "search_code" => "search",
                "hydrate_symbols" => "hydrate",
                "get_definition" => "definition",
                "find_references" => "references",
                "get_call_hierarchy" => "call-hierarchy",
                "get_type_graph" => "type-graph",
                "explore_dependency_graph" => "dependency-graph",
                "refresh_index" => "index refresh",
                "get_index_stats" => "index status",
                _ => return None,
            };
            tool.as_object_mut()?
                .insert("cli_command".to_string(), serde_json::json!(cli_command));
            Some(tool)
        })
        .collect::<Vec<_>>();
    serde_json::json!({
        "ok": true,
        "interface": "cli",
        "version": env!("CARGO_PKG_VERSION"),
        "commands": [
            "ask",
            "search",
            "investigate",
            "hydrate",
            "repo-map",
            "definition",
            "references",
            "call-hierarchy",
            "type-graph",
            "dependency-graph",
            "index",
            "capabilities"
        ],
        "tools": tools,
        "mcp": {
            "supported": true,
            "role": "optional compatibility adapter"
        }
    })
}

fn cli_repo(repo: Option<String>) -> anyhow::Result<String> {
    let current_dir = Utf8PathBuf::from_path_buf(std::env::current_dir()?).map_err(|path| {
        anyhow::anyhow!(
            "current directory contains non-UTF-8 characters: {}",
            path.display()
        )
    })?;
    Ok(resolve_cli_repo(repo, &current_dir).into_string())
}

fn resolve_cli_repo(repo: Option<String>, current_dir: &Utf8Path) -> Utf8PathBuf {
    let requested = repo
        .map(Utf8PathBuf::from)
        .unwrap_or_else(|| current_dir.to_path_buf());
    if requested.is_absolute() {
        requested
    } else {
        current_dir.join(requested)
    }
}

async fn post_cli_query(
    command: &str,
    body: serde_json::Value,
    port_override: Option<u16>,
    runtime: &CliQueryRuntime,
) -> Result<serde_json::Value, CliFailure> {
    post_cli_api(
        command,
        &format!("/api/query/{command}"),
        body,
        port_override,
        runtime,
    )
    .await
}

async fn post_cli_api(
    command: &str,
    path: &str,
    body: serde_json::Value,
    port_override: Option<u16>,
    runtime: &CliQueryRuntime,
) -> Result<serde_json::Value, CliFailure> {
    cli_api_request(
        command,
        reqwest::Method::POST,
        path,
        Some(body),
        port_override,
        runtime,
    )
    .await
}

async fn get_cli_api(
    command: &str,
    path: &str,
    port_override: Option<u16>,
    runtime: &CliQueryRuntime,
) -> Result<serde_json::Value, CliFailure> {
    cli_api_request(
        command,
        reqwest::Method::GET,
        path,
        None,
        port_override,
        runtime,
    )
    .await
}

async fn cli_api_request(
    command: &str,
    method: reqwest::Method,
    path: &str,
    body: Option<serde_json::Value>,
    port_override: Option<u16>,
    runtime: &CliQueryRuntime,
) -> Result<serde_json::Value, CliFailure> {
    let config =
        code_intelligence_mcp_server::config::StandaloneConfig::load(None, port_override, None)
            .map_err(|e| {
                CliFailure::new(
                    command,
                    CliErrorCode::Internal,
                    format!("failed to load daemon config: {e}"),
                    runtime.json,
                    runtime.pretty,
                )
            })?;
    let api_port = config.port.saturating_add(2);
    let host = match config.host.as_str() {
        "0.0.0.0" | "::" => "127.0.0.1",
        other => other,
    };
    let api_base = format!("http://{host}:{api_port}");
    let url = format!("{api_base}{path}");
    let client = reqwest::Client::builder()
        .timeout(runtime.timeout)
        .connect_timeout(std::time::Duration::from_secs(2))
        .build()
        .map_err(|e| {
            CliFailure::new(
                command,
                CliErrorCode::Internal,
                format!("failed to build HTTP client: {e}"),
                runtime.json,
                runtime.pretty,
            )
        })?;
    let mut response = send_cli_api_request(&client, method.clone(), &url, body.as_ref()).await;
    if response
        .as_ref()
        .is_err_and(|error| !error.is_timeout() && !runtime.no_start)
    {
        code_intelligence_mcp_server::install::start_daemon(false).map_err(|error| {
            CliFailure::new(
                command,
                CliErrorCode::DaemonUnavailable,
                format!("automatic daemon start failed: {error}"),
                runtime.json,
                runtime.pretty,
            )
            .with_hint(
                "Run `code-intel install`, or start the Homebrew service with `brew services start code-intelligence-mcp`",
            )
        })?;
        wait_for_cli_api(command, &client, &api_base, runtime).await?;
        response = send_cli_api_request(&client, method, &url, body.as_ref()).await;
    }
    let response =
        response.map_err(|error| cli_transport_failure(command, &url, error, runtime))?;
    let status = response.status();
    let value = response
        .json::<serde_json::Value>()
        .await
        .unwrap_or_else(|_| serde_json::json!({ "error": format!("HTTP {status}") }));
    if !status.is_success() {
        let message = value
            .get("error")
            .and_then(|v| v.as_str())
            .or_else(|| value.get("message").and_then(|v| v.as_str()))
            .map(ToString::to_string)
            .or_else(|| {
                value
                    .get("status")
                    .and_then(|v| v.as_str())
                    .map(|status_name| format!("{status_name}: {value}"))
            })
            .unwrap_or_else(|| format!("daemon query failed with HTTP {status}"));
        let code = classify_daemon_error(status, &message);
        return Err(
            CliFailure::new(command, code, message, runtime.json, runtime.pretty)
                .with_detail("status", serde_json::json!(status.as_u16())),
        );
    }
    Ok(value)
}

async fn send_cli_api_request(
    client: &reqwest::Client,
    method: reqwest::Method,
    url: &str,
    body: Option<&serde_json::Value>,
) -> Result<reqwest::Response, reqwest::Error> {
    let request = client.request(method, url);
    let request = match body {
        Some(body) => request.json(body),
        None => request,
    };
    request.send().await
}

async fn wait_for_cli_api(
    command: &str,
    client: &reqwest::Client,
    api_base: &str,
    runtime: &CliQueryRuntime,
) -> Result<(), CliFailure> {
    let wait_budget = runtime.timeout.min(std::time::Duration::from_secs(10));
    let deadline = tokio::time::Instant::now() + wait_budget;
    let status_url = format!("{api_base}/api/status");
    loop {
        if client
            .get(&status_url)
            .send()
            .await
            .is_ok_and(|response| response.status().is_success())
        {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(CliFailure::new(
                command,
                CliErrorCode::DaemonUnavailable,
                format!(
                    "daemon did not become ready within {}ms",
                    wait_budget.as_millis()
                ),
                runtime.json,
                runtime.pretty,
            )
            .with_hint(
                "Run `code-intel status` and inspect ~/.code-intelligence/logs/server.log",
            ));
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}

fn cli_transport_failure(
    command: &str,
    url: &str,
    error: reqwest::Error,
    runtime: &CliQueryRuntime,
) -> CliFailure {
    let code = if error.is_timeout() {
        CliErrorCode::Timeout
    } else {
        CliErrorCode::DaemonUnavailable
    };
    let hint = if runtime.no_start {
        "Start the daemon manually or retry without --no-start"
    } else {
        "Run `code-intel start` or `code-intel install` first"
    };
    CliFailure::new(
        command,
        code,
        format!("failed to reach Code Intelligence daemon at {url}: {error}"),
        runtime.json,
        runtime.pretty,
    )
    .with_hint(hint)
}

#[derive(Debug, Clone)]
struct CliQueryRuntime {
    timeout: std::time::Duration,
    no_start: bool,
    json: bool,
    pretty: bool,
}

impl CliQueryRuntime {
    fn new(
        command: &'static str,
        timeout: Option<String>,
        no_start: bool,
        json: bool,
        pretty: bool,
    ) -> Result<Self, CliFailure> {
        let timeout = match timeout {
            Some(raw) => parse_cli_duration(&raw).map_err(|message| {
                CliFailure::new(
                    command,
                    CliErrorCode::InvalidArguments,
                    message,
                    json,
                    pretty,
                )
            })?,
            None => std::time::Duration::from_secs(30),
        };
        Ok(Self {
            timeout,
            no_start,
            json,
            pretty,
        })
    }
}

fn parse_cli_duration(raw: &str) -> Result<std::time::Duration, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("timeout duration is required".to_string());
    }
    let (number, multiplier) = if let Some(n) = trimmed.strip_suffix("ms") {
        (n, 1)
    } else if let Some(n) = trimmed.strip_suffix('s') {
        (n, 1_000)
    } else if let Some(n) = trimmed.strip_suffix('m') {
        (n, 60_000)
    } else {
        (trimmed, 1_000)
    };
    let value = number
        .parse::<u64>()
        .map_err(|_| format!("invalid timeout duration: {raw}"))?;
    if value == 0 {
        return Err("timeout duration must be greater than zero".to_string());
    }
    Ok(std::time::Duration::from_millis(
        value.saturating_mul(multiplier),
    ))
}

fn classify_daemon_error(status: reqwest::StatusCode, message: &str) -> CliErrorCode {
    let lower = message.to_ascii_lowercase();
    if lower.contains("workspace") || lower.contains("repo") {
        CliErrorCode::WorkspaceUnavailable
    } else if status.is_client_error() {
        CliErrorCode::InvalidArguments
    } else {
        CliErrorCode::Internal
    }
}

fn fail_on_empty_results(
    command: &'static str,
    value: &serde_json::Value,
    runtime: &CliQueryRuntime,
) -> Result<(), CliFailure> {
    let empty = match command {
        "search" => value
            .pointer("/result/hits")
            .and_then(|v| v.as_array())
            .map(|hits| hits.is_empty())
            .unwrap_or(false),
        "hydrate" => value
            .pointer("/result/count")
            .and_then(|v| v.as_u64())
            .map(|count| count == 0)
            .unwrap_or(false),
        "definition" | "references" => value
            .pointer("/result/count")
            .and_then(|v| v.as_u64())
            .map(|count| count == 0)
            .unwrap_or(false),
        "call-hierarchy" | "type-graph" | "dependency-graph" => value
            .pointer("/result/nodes")
            .and_then(|v| v.as_array())
            .map(|nodes| nodes.is_empty())
            .unwrap_or(false),
        _ => false,
    };
    if empty {
        Err(CliFailure::new(
            command,
            CliErrorCode::NoResults,
            "query completed but returned no results",
            runtime.json,
            runtime.pretty,
        )
        .with_detail("result", value.get("result").cloned().unwrap_or_default()))
    } else {
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CliErrorCode {
    Internal,
    InvalidArguments,
    DaemonUnavailable,
    WorkspaceUnavailable,
    NoResults,
    Timeout,
}

impl CliErrorCode {
    fn as_str(self) -> &'static str {
        match self {
            Self::Internal => "internal",
            Self::InvalidArguments => "invalid_arguments",
            Self::DaemonUnavailable => "daemon_unavailable",
            Self::WorkspaceUnavailable => "workspace_unavailable",
            Self::NoResults => "no_results",
            Self::Timeout => "timeout",
        }
    }

    fn exit_code(self) -> i32 {
        match self {
            Self::Internal => 1,
            Self::InvalidArguments => 2,
            Self::DaemonUnavailable => 3,
            Self::WorkspaceUnavailable => 4,
            Self::NoResults => 5,
            Self::Timeout => 124,
        }
    }
}

#[derive(Debug, Clone)]
struct CliFailure {
    command: String,
    code: CliErrorCode,
    message: String,
    hint: Option<String>,
    details: serde_json::Map<String, serde_json::Value>,
    json: bool,
    pretty: bool,
}

impl CliFailure {
    fn new(
        command: impl Into<String>,
        code: CliErrorCode,
        message: impl Into<String>,
        json: bool,
        pretty: bool,
    ) -> Self {
        Self {
            command: command.into(),
            code,
            message: message.into(),
            hint: None,
            details: serde_json::Map::new(),
            json,
            pretty,
        }
    }

    fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    fn with_detail(mut self, key: impl Into<String>, value: serde_json::Value) -> Self {
        self.details.insert(key.into(), value);
        self
    }

    fn envelope(&self) -> serde_json::Value {
        let mut error = serde_json::Map::new();
        error.insert("code".to_string(), serde_json::json!(self.code.as_str()));
        error.insert("message".to_string(), serde_json::json!(self.message));
        if let Some(hint) = &self.hint {
            error.insert("hint".to_string(), serde_json::json!(hint));
        }
        for (key, value) in &self.details {
            error.insert(key.clone(), value.clone());
        }
        serde_json::json!({
            "ok": false,
            "command": self.command,
            "error": error,
        })
    }

    fn exit_code(&self) -> i32 {
        self.code.exit_code()
    }

    fn emit(&self) {
        if self.json || self.pretty {
            let rendered = if self.pretty {
                serde_json::to_string_pretty(&self.envelope())
            } else {
                serde_json::to_string(&self.envelope())
            };
            match rendered {
                Ok(line) => println!("{line}"),
                Err(_) => eprintln!("{self}"),
            }
        } else {
            eprintln!("{self}");
            if let Some(hint) = &self.hint {
                eprintln!("hint: {hint}");
            }
        }
    }
}

impl std::fmt::Display for CliFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} failed: {}", self.command, self.message)
    }
}

impl std::error::Error for CliFailure {}

fn print_cli_query_response(
    value: &serde_json::Value,
    json: bool,
    pretty: bool,
) -> anyhow::Result<()> {
    if pretty || !json {
        println!("{}", serde_json::to_string_pretty(value)?);
    } else {
        println!("{}", serde_json::to_string(value)?);
    }
    Ok(())
}

async fn run_standalone(
    host: Option<&str>,
    port: Option<u16>,
    discovery_port: Option<u16>,
    log_broadcaster: code_intelligence_mcp_server::log_broadcast::LogBroadcaster,
) -> SdkResult<()> {
    let standalone_config =
        code_intelligence_mcp_server::config::StandaloneConfig::load(host, port, discovery_port)
            .map_err(|e| McpSdkError::Internal {
                description: e.to_string(),
            })?;

    if matches!(
        standalone_config.embeddings_backend,
        code_intelligence_mcp_server::config::EmbeddingsBackend::LlamaCpp
    ) || standalone_config.reranker_enabled
    {
        ensure_native_llama_available().map_err(|e| McpSdkError::Internal {
            description: format!("Native model configuration is unavailable: {e}"),
        })?;
    }

    // Ensure data directories exist
    let data_dir = &standalone_config.data_dir;
    std::fs::create_dir_all(data_dir.join("repos").as_std_path()).map_err(|e| {
        McpSdkError::Internal {
            description: format!("Failed to create data dir: {}", e),
        }
    })?;
    std::fs::create_dir_all(data_dir.join("logs").as_std_path()).map_err(|e| {
        McpSdkError::Internal {
            description: format!("Failed to create logs dir: {}", e),
        }
    })?;

    // Create shared embedder (loaded once, shared across all repos).
    // For llamacpp backend, use DeferredEmbedder so the HTTP server starts immediately.
    let (embedder, standalone_deferred_slot): EmbedderWithSlot =
        match standalone_config.embeddings_backend {
            code_intelligence_mcp_server::config::EmbeddingsBackend::Hash => {
                let base = create_embedder(
                    standalone_config.embeddings_backend,
                    standalone_config.embeddings_model_dir.as_deref(),
                    standalone_config.embeddings_device,
                    standalone_config.hash_embedding_dim,
                )
                .map_err(|e| McpSdkError::Internal {
                    description: format!("Failed to create embedder: {}", e),
                })?;
                let e = maybe_truncate(base, standalone_config.embedding_truncate_dim).map_err(
                    |e| McpSdkError::Internal {
                        description: format!("Failed to create truncating embedder: {}", e),
                    },
                )?;
                info!("Created hash embedder with dimension: {}", e.dim());
                (Arc::new(SharedEmbedder::new(e)), None)
            }
            code_intelligence_mcp_server::config::EmbeddingsBackend::LlamaCpp => {
                let dim = default_embedding_dim(
                    standalone_config.embeddings_backend,
                    standalone_config.hash_embedding_dim,
                    standalone_config.embedding_truncate_dim,
                    None,
                );
                let deferred = DeferredEmbedder::new(dim);
                let slot = deferred.inner_slot();
                info!(
                    dim,
                    "Created deferred embedder — model will load in background"
                );
                (
                    Arc::new(SharedEmbedder::new(Box::new(deferred))),
                    Some(slot),
                )
            }
        };

    // Create registry and session manager
    let registry = code_intelligence_mcp_server::registry::RepoRegistry::new(
        data_dir.join("repos/registry.json"),
        data_dir.join("repos"),
    );

    let job_registry = code_intelligence_mcp_server::server::jobs::new_job_registry();
    code_intelligence_mcp_server::server::jobs::spawn_job_eviction_loop(job_registry.clone());

    // Build the shared cross-encoder reranker. Off by default (the model is
    // ~600MB GPU-resident and its quality benefit is unproven). When enabled we
    // hand the SessionManager a DeferredReranker immediately and load the real
    // model in a background task (mirrors the deferred embedder), so the HTTP
    // server is up before the download/load finishes. Searches run BM25+vector
    // (no reordering) until the slot is filled.
    let (reranker, reranker_slot): (Option<Arc<dyn Reranker>>, Option<RerankerSlot>) =
        if standalone_config.reranker_enabled {
            let deferred = DeferredReranker::new(RERANKER_TOP_K);
            let slot = deferred.inner_slot();
            info!("Reranker enabled — model will load in background");
            (Some(Arc::new(deferred)), Some(slot))
        } else {
            (None, None)
        };

    let session_manager = code_intelligence_mcp_server::session::SessionManager::new(
        standalone_config.clone(),
        registry,
        embedder,
        Some(job_registry.clone()),
        reranker,
    )
    .await
    .map_err(|e| McpSdkError::Internal {
        description: e.to_string(),
    })?;
    let session_manager = Arc::new(session_manager);
    session_manager.spawn_eviction_loop();

    let server_details = InitializeResult {
        server_info: Implementation {
            name: "code-intelligence".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            title: Some("Code Intelligence MCP (Standalone)".into()),
            description: Some("Multi-repo code intelligence server".into()),
            icons: vec![],
            website_url: None,
        },
        capabilities: ServerCapabilities {
            tools: Some(ServerCapabilitiesTools { list_changed: None }),
            tasks: Some(code_intelligence_mcp_server::server::task_capabilities()),
            ..Default::default()
        },
        protocol_version: ProtocolVersion::V2025_11_25.into(),
        instructions: Some(code_intelligence_mcp_server::server::server_instructions().into()),
        meta: None,
    };

    let session_repos = code_intelligence_mcp_server::server::standalone::new_session_repos();
    let pending_repos = code_intelligence_mcp_server::server::mcp_proxy::new_pending_repos();
    let bound_repos = code_intelligence_mcp_server::server::mcp_proxy::new_bound_repos();
    code_intelligence_mcp_server::server::standalone::spawn_session_eviction_loop(
        session_repos.clone(),
        bound_repos.clone(),
    );
    let handler = code_intelligence_mcp_server::server::standalone::StandaloneHandler::new(
        session_manager.clone(),
        server_details.clone(),
        session_repos.clone(),
        pending_repos.clone(),
        bound_repos.clone(),
    );
    let bind_host = standalone_config.host.clone();
    let bind_port = standalone_config.port;
    // The SDK owns the axum router and exposes no hook to read the request
    // URI from inside the MCP transport. We bind it to an internal loopback
    // port and run a small proxy on the public port that captures `?repo=`
    // URL bindings on the way through. Offset by +100 so it never collides
    // with discovery/api ports (+1 and +2).
    let internal_mcp_port = bind_port.saturating_add(100);

    // Use SDK's hyper server for Streamable HTTP transport
    use rust_mcp_sdk::mcp_server::{hyper_server, HyperServerOptions, ToMcpServerHandler};
    let server = hyper_server::create_server(
        server_details,
        handler.to_mcp_server_handler(),
        HyperServerOptions {
            host: "127.0.0.1".to_string(),
            port: internal_mcp_port,
            task_store: Some(Arc::new(rust_mcp_sdk::task_store::InMemoryTaskStore::<
                rust_mcp_sdk::schema::schema_utils::ClientJsonrpcRequest,
                rust_mcp_sdk::schema::schema_utils::ResultFromServer,
            >::new(None))),
            client_task_store: None,
            ..Default::default()
        },
    );

    // Spawn background embedder loading for LlamaCpp backend in standalone mode.
    if let Some(slot) = standalone_deferred_slot {
        let model_dir = standalone_config.embeddings_model_dir.clone();
        let device = standalone_config.embeddings_device;
        let hash_dim = standalone_config.hash_embedding_dim;
        let truncate_dim = standalone_config.embedding_truncate_dim;
        tokio::spawn(async move {
            info!("Starting background embedding model download/load (standalone)...");
            let result = tokio::task::spawn_blocking(move || {
                let base = create_embedder(
                    code_intelligence_mcp_server::config::EmbeddingsBackend::LlamaCpp,
                    model_dir.as_deref(),
                    device,
                    hash_dim,
                )?;
                let embedder = maybe_truncate(base, truncate_dim)?;
                Ok::<_, anyhow::Error>(embedder)
            })
            .await;
            match result {
                Ok(Ok(real_embedder)) => {
                    let mut guard = slot.lock().expect("DeferredEmbedder mutex poisoned");
                    *guard = Some(real_embedder);
                    info!("Embedding model loaded — vector search is now available (standalone)");
                }
                Ok(Err(e)) => {
                    error!("Failed to load embedding model: {}. Vector search will remain unavailable.", e);
                }
                Err(e) => {
                    error!("Embedding model loading task panicked: {}. Vector search will remain unavailable.", e);
                }
            }
        });
    }

    // Spawn background reranker model download/load when enabled. Fills the
    // DeferredReranker slot handed to the SessionManager above.
    if let Some(slot) = reranker_slot {
        let cache_dir = standalone_config.data_dir.join("reranker-cache");
        tokio::spawn(async move {
            info!("Starting background reranker model download/load...");
            let result = tokio::task::spawn_blocking(move || {
                create_reranker(true, None, Some(cache_dir.as_path()), RERANKER_TOP_K)
            })
            .await;
            match result {
                Ok(Ok(Some(real_reranker))) => {
                    let mut guard = slot.lock().expect("DeferredReranker mutex poisoned");
                    *guard = Some(real_reranker);
                    info!("Reranker model loaded — cross-encoder reranking is now active");
                }
                Ok(Ok(None)) => {
                    error!("Reranker enabled but model could not be created. Reranking stays off.");
                }
                Ok(Err(e)) => {
                    error!("Failed to load reranker model: {}. Reranking stays off.", e);
                }
                Err(e) => {
                    error!(
                        "Reranker loading task panicked: {}. Reranking stays off.",
                        e
                    );
                }
            }
        });
    }

    // Spawn discovery endpoint (always in standalone mode)
    {
        let discovery_port = standalone_config
            .discovery_port
            .unwrap_or(bind_port.saturating_add(1));
        if let Err(e) = code_intelligence_mcp_server::server::discovery::spawn_discovery_server(
            &bind_host,
            bind_port,
            discovery_port,
        )
        .await
        {
            error!(
                "Failed to start discovery server: {}. Discovery endpoint will not be available.",
                e
            );
        }
    }

    // Spawn the MCP proxy on the public port. It forwards POST/GET/DELETE
    // /mcp to the internal SDK listener (port +100), reading `?repo=` URL
    // bindings on the way through and pairing them with the SDK-assigned
    // mcp-session-id on the way back.
    {
        let pr = pending_repos.clone();
        let br = bound_repos.clone();
        if let Err(e) = code_intelligence_mcp_server::server::mcp_proxy::spawn_mcp_proxy(
            &bind_host,
            bind_port,
            internal_mcp_port,
            "/mcp",
            pr,
            br,
        )
        .await
        {
            error!(
                "Failed to start MCP proxy: {}. ?repo= URL binding will not be available.",
                e
            );
        }
    }

    // Spawn JSON API endpoint on mcp_port + 2 (or +3 if collision with discovery).
    {
        let api_port = bind_port.saturating_add(2);
        if let Err(e) = code_intelligence_mcp_server::server::api::spawn_api_server(
            &bind_host,
            api_port,
            session_manager.clone(),
            session_repos.clone(),
            log_broadcaster.clone(),
            job_registry.clone(),
        )
        .await
        {
            error!(
                "Failed to start API server: {}. /api/* endpoints will not be available.",
                e
            );
        }
    }

    info!(
        host = %bind_host,
        port = bind_port,
        data_dir = %standalone_config.data_dir,
        "Starting standalone server on http://{}:{}",
        bind_host, bind_port,
    );

    server.start().await
}

#[cfg(test)]
mod cli_query_contract_tests {
    use super::*;

    #[test]
    fn cli_failure_envelope_matches_contract() {
        let failure = CliFailure::new(
            "search",
            CliErrorCode::DaemonUnavailable,
            "Code Intelligence daemon is not running",
            true,
            false,
        )
        .with_hint("Run `code-intelligence-mcp-server start` first");

        let envelope = failure.envelope();
        assert_eq!(envelope["ok"], false);
        assert_eq!(envelope["command"], "search");
        assert_eq!(envelope["error"]["code"], "daemon_unavailable");
        assert_eq!(
            envelope["error"]["message"],
            "Code Intelligence daemon is not running"
        );
        assert_eq!(
            envelope["error"]["hint"],
            "Run `code-intelligence-mcp-server start` first"
        );
        assert_eq!(failure.exit_code(), 3);
    }

    #[test]
    fn cli_failure_exit_codes_are_stable() {
        assert_eq!(CliErrorCode::Internal.exit_code(), 1);
        assert_eq!(CliErrorCode::InvalidArguments.exit_code(), 2);
        assert_eq!(CliErrorCode::DaemonUnavailable.exit_code(), 3);
        assert_eq!(CliErrorCode::WorkspaceUnavailable.exit_code(), 4);
        assert_eq!(CliErrorCode::NoResults.exit_code(), 5);
        assert_eq!(CliErrorCode::Timeout.exit_code(), 124);
    }

    #[test]
    fn cli_capabilities_advertise_cli_tools_without_mcp_binding_commands() {
        let value = cli_capabilities_value();
        assert_eq!(value["interface"], "cli");
        assert!(value["commands"]
            .as_array()
            .unwrap()
            .iter()
            .any(|command| command == "definition"));
        let tool_names = value["tools"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|tool| tool["name"].as_str())
            .collect::<Vec<_>>();
        assert!(tool_names.contains(&"ask_code"));
        assert!(!tool_names.contains(&"bind_workspace"));
        assert!(!tool_names.contains(&"approve_indexing"));
    }

    #[test]
    fn explicit_relative_cli_repo_is_resolved_against_client_directory() {
        let current_dir = Utf8Path::new("/Users/example/project");

        let resolved = resolve_cli_repo(Some(".".to_string()), current_dir);

        assert!(resolved.is_absolute());
        assert_eq!(resolved, current_dir.join("."));
        assert_ne!(resolved, Utf8PathBuf::from("."));
    }

    #[test]
    fn absolute_cli_repo_is_preserved() {
        let current_dir = Utf8Path::new("/Users/example/project");
        let absolute = "/Volumes/code/another-project";

        assert_eq!(
            resolve_cli_repo(Some(absolute.to_string()), current_dir),
            Utf8PathBuf::from(absolute)
        );
        assert_eq!(resolve_cli_repo(None, current_dir), current_dir);
    }

    #[test]
    fn empty_navigation_results_use_no_results_failure() {
        let runtime = CliQueryRuntime {
            timeout: std::time::Duration::from_secs(1),
            no_start: true,
            json: true,
            pretty: false,
        };
        let cases = [
            (
                "definition",
                serde_json::json!({ "result": { "count": 0 } }),
            ),
            (
                "references",
                serde_json::json!({ "result": { "count": 0 } }),
            ),
            (
                "call-hierarchy",
                serde_json::json!({ "result": { "nodes": [] } }),
            ),
            (
                "type-graph",
                serde_json::json!({ "result": { "nodes": [] } }),
            ),
            (
                "dependency-graph",
                serde_json::json!({ "result": { "nodes": [] } }),
            ),
        ];

        for (command, value) in cases {
            let failure = fail_on_empty_results(command, &value, &runtime)
                .expect_err("empty navigation result should fail");
            assert_eq!(failure.code, CliErrorCode::NoResults, "{command}");
            assert_eq!(failure.exit_code(), 5, "{command}");
        }
    }

    #[test]
    fn graph_root_without_edges_is_still_a_result() {
        let runtime = CliQueryRuntime {
            timeout: std::time::Duration::from_secs(1),
            no_start: true,
            json: true,
            pretty: false,
        };
        let value = serde_json::json!({
            "result": {
                "nodes": [{ "name": "LeafType" }],
                "edges": []
            }
        });

        fail_on_empty_results("type-graph", &value, &runtime)
            .expect("a resolved graph root is a successful result");
    }
}
