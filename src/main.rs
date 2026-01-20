use async_trait::async_trait;
use rust_mcp_sdk::{
    error::{McpSdkError, SdkResult},
    macros,
    mcp_server::{server_runtime, McpServerOptions, ServerHandler, ToMcpServerHandler},
    schema::{
        CallToolError, CallToolRequestParams, CallToolResult, Implementation, InitializeResult,
        ListToolsResult, PaginatedRequestParams, ProtocolVersion, RpcError, ServerCapabilities,
        ServerCapabilitiesTools,
    },
    McpServer, StdioTransport, TransportOptions,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::{debug, error, info};
use tracing_subscriber::EnvFilter;

use code_intelligence_mcp_server::config::{Config, EmbeddingsBackend};
use code_intelligence_mcp_server::embeddings::{hash::HashEmbedder, Embedder};
use code_intelligence_mcp_server::indexer::pipeline::IndexPipeline;
use code_intelligence_mcp_server::retrieval::assembler::{ContextAssembler, FormatMode};
use code_intelligence_mcp_server::retrieval::Retriever;
use code_intelligence_mcp_server::storage::sqlite::{SqliteStore, SymbolRow};
use code_intelligence_mcp_server::storage::tantivy::TantivyIndex;
use code_intelligence_mcp_server::storage::vector::LanceDbStore;
use serde::de::DeserializeOwned;
use serde_json::json;
use tokio::sync::Mutex;

fn wants_help(args: &[String]) -> bool {
    args.iter()
        .skip(1)
        .any(|a| a == "-h" || a == "--help" || a == "help")
}

fn wants_version(args: &[String]) -> bool {
    args.iter()
        .skip(1)
        .any(|a| a == "-V" || a == "--version" || a == "version")
}

fn print_help() {
    println!("code-intelligence-mcp-server");
    println!();
    println!("MCP server over stdio for local code intelligence (index + search + context).");
    println!();
    println!("Usage:");
    println!("  code-intelligence-mcp-server");
    println!("  code-intelligence-mcp-server --help");
    println!("  code-intelligence-mcp-server --version");
    println!();
    println!("Required env:");
    println!("  BASE_DIR=/absolute/path/to/repo");
    println!();
    println!("Common env (defaults shown):");
    println!("  EMBEDDINGS_MODEL_DIR=/path/to/cache   (default: ./.cimcp/embeddings-cache)");
    println!("  EMBEDDINGS_BACKEND=fastembed|hash     (default: fastembed)");
    println!("  EMBEDDINGS_MODEL_REPO=org/repo       (default: BAAI/bge-base-en-v1.5)");
    println!("                                       (supported: BAAI/bge-base-en-v1.5, BAAI/bge-small-en-v1.5,");
    println!("                                        sentence-transformers/all-MiniLM-L6-v2, jinaai/jina-embeddings-v2-base-en)");
    println!("  EMBEDDINGS_DEVICE=cpu|metal          (default: cpu; fastembed handles acceleration automatically)");
    println!("  EMBEDDING_BATCH_SIZE=32");
    println!("  DB_PATH=./.cimcp/code-intelligence.db       (resolved under BASE_DIR if relative)");
    println!("  VECTOR_DB_PATH=./.cimcp/vectors             (resolved under BASE_DIR if relative)");
    println!("  TANTIVY_INDEX_PATH=./.cimcp/tantivy-index   (resolved under BASE_DIR if relative)");
    println!("  MAX_CONTEXT_BYTES=200000");
    println!("  WATCH_MODE=true|false                (default: true)");
    println!("  REPO_ROOTS=/path/a,/path/b           (default: BASE_DIR only)");
    println!();
    println!("Embeddings auto-detection:");
    println!("  - Defaults to fastembed (using BGE Base v1.5).");
    println!("  - Set EMBEDDINGS_BACKEND=hash to use deterministic hashing (no model).");
    println!();
    println!("Tools:");
    println!("  search_code, refresh_index, get_definition, find_references, get_file_symbols,");
    println!("  get_call_hierarchy, get_type_graph, get_usage_examples, get_index_stats, get_similarity_cluster");
}

fn print_version() {
    println!("{}", env!("CARGO_PKG_VERSION"));
}

#[macros::mcp_tool(
    name = "search_code",
    description = "Search codebase for symbols and return assembled context."
)]
#[derive(Debug, Clone, Deserialize, Serialize, macros::JsonSchema)]
pub struct SearchCodeTool {
    pub query: String,
    pub limit: Option<u32>,
    pub exported_only: Option<bool>,
}

#[macros::mcp_tool(
    name = "refresh_index",
    description = "Re-index the codebase or specific files."
)]
#[derive(Debug, Clone, Deserialize, Serialize, macros::JsonSchema)]
pub struct RefreshIndexTool {
    pub files: Option<Vec<String>>,
}

#[macros::mcp_tool(
    name = "get_definition",
    description = "Get full definition(s) for a symbol by name with disambiguation support."
)]
#[derive(Debug, Clone, Deserialize, Serialize, macros::JsonSchema)]
pub struct GetDefinitionTool {
    pub symbol_name: String,
    pub file: Option<String>,
    pub limit: Option<u32>,
}

#[macros::mcp_tool(
    name = "find_references",
    description = "Find imports/uses/calls of a symbol across the indexed graph."
)]
#[derive(Debug, Clone, Deserialize, Serialize, macros::JsonSchema)]
pub struct FindReferencesTool {
    pub symbol_name: String,
    pub reference_type: Option<String>,
    pub limit: Option<u32>,
}

#[macros::mcp_tool(
    name = "get_file_symbols",
    description = "List symbols defined in a file (no full definitions)."
)]
#[derive(Debug, Clone, Deserialize, Serialize, macros::JsonSchema)]
pub struct GetFileSymbolsTool {
    pub file_path: String,
    pub exported_only: Option<bool>,
}

#[macros::mcp_tool(
    name = "get_call_hierarchy",
    description = "Return a best-effort call hierarchy rooted at a symbol."
)]
#[derive(Debug, Clone, Deserialize, Serialize, macros::JsonSchema)]
pub struct GetCallHierarchyTool {
    pub symbol_name: String,
    pub direction: Option<String>,
    pub depth: Option<u32>,
    pub limit: Option<u32>,
}

#[macros::mcp_tool(
    name = "get_type_graph",
    description = "Return type relationships for a symbol (extends/implements/aliases)."
)]
#[derive(Debug, Clone, Deserialize, Serialize, macros::JsonSchema)]
pub struct GetTypeGraphTool {
    pub symbol_name: String,
    pub depth: Option<u32>,
    pub limit: Option<u32>,
}

#[macros::mcp_tool(
    name = "get_usage_examples",
    description = "Return extracted usage examples for a symbol from the indexed repo."
)]
#[derive(Debug, Clone, Deserialize, Serialize, macros::JsonSchema)]
pub struct GetUsageExamplesTool {
    pub symbol_name: String,
    pub limit: Option<u32>,
}

#[macros::mcp_tool(
    name = "get_index_stats",
    description = "Return index statistics (files, symbols, edges, last updated)."
)]
#[derive(Debug, Clone, Deserialize, Serialize, macros::JsonSchema)]
pub struct GetIndexStatsTool {}

#[macros::mcp_tool(
    name = "explore_dependency_graph",
    description = "Explore dependencies upstream/downstream/bidirectional from a symbol."
)]
#[derive(Debug, Clone, Deserialize, Serialize, macros::JsonSchema)]
pub struct ExploreDependencyGraphTool {
    pub symbol_name: String,
    pub direction: Option<String>,
    pub depth: Option<u32>,
    pub limit: Option<u32>,
}

#[macros::mcp_tool(
    name = "get_similarity_cluster",
    description = "Return symbols in the same similarity cluster as the given symbol."
)]
#[derive(Debug, Clone, Deserialize, Serialize, macros::JsonSchema)]
pub struct GetSimilarityClusterTool {
    pub symbol_name: String,
    pub limit: Option<u32>,
}

#[macros::mcp_tool(
    name = "hydrate_symbols",
    description = "Hydrate full context for a set of symbol ids."
)]
#[derive(Debug, Clone, Deserialize, Serialize, macros::JsonSchema)]
pub struct HydrateSymbolsTool {
    pub ids: Vec<String>,
    pub mode: Option<String>,
}

#[derive(Clone)]
struct AppState {
    config: Arc<Config>,
    indexer: IndexPipeline,
    retriever: Retriever,
}

#[derive(Clone)]
struct CodeIntelligenceHandler {
    state: Arc<AppState>,
}

fn parse_tool_args<T: DeserializeOwned>(
    params: &CallToolRequestParams,
) -> std::result::Result<T, CallToolError> {
    let args = params.arguments.clone().unwrap_or_default();
    let args = serde_json::Value::Object(args);
    serde_json::from_value(args)
        .map_err(|err| CallToolError::invalid_arguments(&params.name, Some(err.to_string())))
}

fn tool_internal_error(err: anyhow::Error) -> CallToolError {
    CallToolError::from_message(err.to_string())
}

#[async_trait]
impl ServerHandler for CodeIntelligenceHandler {
    async fn handle_list_tools_request(
        &self,
        _request: Option<PaginatedRequestParams>,
        _runtime: Arc<dyn McpServer>,
    ) -> std::result::Result<ListToolsResult, RpcError> {
        Ok(ListToolsResult {
            tools: vec![
                SearchCodeTool::tool(),
                RefreshIndexTool::tool(),
                GetDefinitionTool::tool(),
                FindReferencesTool::tool(),
                GetFileSymbolsTool::tool(),
                GetCallHierarchyTool::tool(),
                ExploreDependencyGraphTool::tool(),
                GetTypeGraphTool::tool(),
                GetUsageExamplesTool::tool(),
                GetIndexStatsTool::tool(),
                HydrateSymbolsTool::tool(),
            ],
            meta: None,
            next_cursor: None,
        })
    }

    async fn handle_call_tool_request(
        &self,
        params: CallToolRequestParams,
        _runtime: Arc<dyn McpServer>,
    ) -> std::result::Result<CallToolResult, CallToolError> {
        match params.name.as_str() {
            "refresh_index" => {
                let tool: RefreshIndexTool = parse_tool_args(&params)?;

                let stats = if let Some(files) = tool.files {
                    let paths = files
                        .into_iter()
                        .map(|p| {
                            self.state
                                .config
                                .normalize_path_to_base(std::path::Path::new(&p))
                        })
                        .collect::<Result<Vec<_>, _>>()
                        .map_err(tool_internal_error)?;
                    self.state.indexer.index_paths(&paths).await
                } else {
                    self.state.indexer.index_all().await
                }
                .map_err(tool_internal_error)?;

                Ok(CallToolResult::text_content(vec![
                    serde_json::to_string_pretty(&json!({
                        "ok": true,
                        "stats": stats,
                    }))
                    .unwrap_or_else(|_| "{\"ok\":true}".to_string())
                    .into(),
                ]))
            }
            "search_code" => {
                let tool: SearchCodeTool = parse_tool_args(&params)?;
                let limit = tool.limit.unwrap_or(5).max(1) as usize;
                let exported_only = tool.exported_only.unwrap_or(false);

                let resp = self
                    .state
                    .retriever
                    .search(&tool.query, limit, exported_only)
                    .await
                    .map_err(tool_internal_error)?;

                Ok(CallToolResult::text_content(vec![
                    serde_json::to_string_pretty(&resp)
                        .unwrap_or_else(|_| "{\"ok\":true}".to_string())
                        .into(),
                ]))
            }
            "get_definition" => {
                let tool: GetDefinitionTool = parse_tool_args(&params)?;
                let limit = tool.limit.unwrap_or(10).max(1) as usize;

                let sqlite =
                    SqliteStore::open(&self.state.config.db_path).map_err(tool_internal_error)?;
                sqlite.init().map_err(tool_internal_error)?;

                let rows = sqlite
                    .search_symbols_by_exact_name(&tool.symbol_name, tool.file.as_deref(), limit)
                    .map_err(tool_internal_error)?;

                let context = self
                    .state
                    .retriever
                    .assemble_definitions(&rows)
                    .map_err(tool_internal_error)?;

                Ok(CallToolResult::text_content(vec![
                    serde_json::to_string_pretty(&json!({
                        "symbol_name": tool.symbol_name,
                        "count": rows.len(),
                        "definitions": rows,
                        "context": context,
                    }))
                    .unwrap_or_else(|_| "{\"ok\":true}".to_string())
                    .into(),
                ]))
            }
            "get_file_symbols" => {
                let tool: GetFileSymbolsTool = parse_tool_args(&params)?;
                let exported_only = tool.exported_only.unwrap_or(false);

                let sqlite =
                    SqliteStore::open(&self.state.config.db_path).map_err(tool_internal_error)?;
                sqlite.init().map_err(tool_internal_error)?;

                let rows = sqlite
                    .list_symbol_headers_by_file(&tool.file_path, exported_only)
                    .map_err(tool_internal_error)?;

                Ok(CallToolResult::text_content(vec![
                    serde_json::to_string_pretty(&json!({
                        "file_path": tool.file_path,
                        "count": rows.len(),
                        "symbols": rows,
                    }))
                    .unwrap_or_else(|_| "{\"ok\":true}".to_string())
                    .into(),
                ]))
            }
            "get_index_stats" => {
                let _tool: GetIndexStatsTool =
                    parse_tool_args(&params).unwrap_or(GetIndexStatsTool {});

                let sqlite =
                    SqliteStore::open(&self.state.config.db_path).map_err(tool_internal_error)?;
                sqlite.init().map_err(tool_internal_error)?;

                let symbols = sqlite.count_symbols().map_err(tool_internal_error)?;
                let edges = sqlite.count_edges().map_err(tool_internal_error)?;
                let last_updated = sqlite
                    .most_recent_symbol_update()
                    .map_err(tool_internal_error)?;
                let latest_index_run = sqlite.latest_index_run().map_err(tool_internal_error)?;
                let latest_search_run = sqlite.latest_search_run().map_err(tool_internal_error)?;

                Ok(CallToolResult::text_content(vec![
                    serde_json::to_string_pretty(&json!({
                        "base_dir": self.state.config.base_dir,
                        "symbols": symbols,
                        "edges": edges,
                        "last_updated_unix_s": last_updated,
                        "latest_index_run": latest_index_run,
                        "latest_search_run": latest_search_run,
                    }))
                    .unwrap_or_else(|_| "{\"ok\":true}".to_string())
                    .into(),
                ]))
            }
            "hydrate_symbols" => {
                let tool: HydrateSymbolsTool = parse_tool_args(&params)?;

                let sqlite =
                    SqliteStore::open(&self.state.config.db_path).map_err(tool_internal_error)?;
                sqlite.init().map_err(tool_internal_error)?;

                let mut rows = Vec::new();
                let mut missing = Vec::new();
                for id in tool.ids {
                    match sqlite.get_symbol_by_id(&id).map_err(tool_internal_error)? {
                        Some(row) => rows.push(row),
                        None => missing.push(id),
                    }
                }

                let mode = match tool.mode.as_deref() {
                    Some("full") => FormatMode::Full,
                    _ => FormatMode::Default,
                };

                let assembler = ContextAssembler::new(self.state.config.clone());
                let (context, context_items) = assembler
                    .format_context_with_mode(&sqlite, &rows, &[], &[], mode)
                    .map_err(tool_internal_error)?;

                Ok(CallToolResult::text_content(vec![
                    serde_json::to_string_pretty(&json!({
                        "count": rows.len(),
                        "missing_ids": missing,
                        "context": context,
                        "context_items": context_items,
                    }))
                    .unwrap_or_else(|_| "{}".to_string())
                    .into(),
                ]))
            }
            "explore_dependency_graph" => {
                let tool: ExploreDependencyGraphTool = parse_tool_args(&params)?;
                let depth = tool.depth.unwrap_or(2) as usize;
                let limit = tool.limit.unwrap_or(200).max(1) as usize;
                let direction = tool.direction.unwrap_or_else(|| "downstream".to_string());

                let sqlite =
                    SqliteStore::open(&self.state.config.db_path).map_err(tool_internal_error)?;
                sqlite.init().map_err(tool_internal_error)?;

                let roots = sqlite
                    .search_symbols_by_exact_name(&tool.symbol_name, None, 10)
                    .map_err(tool_internal_error)?;
                let root = roots.first().cloned();

                let Some(root) = root else {
                    return Ok(CallToolResult::text_content(vec![
                        serde_json::to_string_pretty(&json!({
                            "symbol_name": tool.symbol_name,
                            "direction": direction,
                            "depth": depth,
                            "nodes": [],
                            "edges": [],
                        }))
                        .unwrap_or_else(|_| "{}".to_string())
                        .into(),
                    ]));
                };

                let graph = build_dependency_graph(&sqlite, &root, &direction, depth, limit)
                    .map_err(tool_internal_error)?;

                Ok(CallToolResult::text_content(vec![
                    serde_json::to_string_pretty(&graph)
                        .unwrap_or_else(|_| "{}".to_string())
                        .into(),
                ]))
            }
            "get_similarity_cluster" => {
                let tool: GetSimilarityClusterTool = parse_tool_args(&params)?;
                let limit = tool.limit.unwrap_or(20).max(1) as usize;

                let sqlite =
                    SqliteStore::open(&self.state.config.db_path).map_err(tool_internal_error)?;
                sqlite.init().map_err(tool_internal_error)?;

                let roots = sqlite
                    .search_symbols_by_exact_name(&tool.symbol_name, None, 10)
                    .map_err(tool_internal_error)?;
                let root = roots.first().cloned();

                let Some(root) = root else {
                    return Ok(CallToolResult::text_content(vec![
                        serde_json::to_string_pretty(&json!({
                            "symbol_name": tool.symbol_name,
                            "cluster_key": null,
                            "count": 0,
                            "symbols": [],
                        }))
                        .unwrap_or_else(|_| "{\"ok\":true}".to_string())
                        .into(),
                    ]));
                };

                let cluster_key = sqlite
                    .get_similarity_cluster_key(&root.id)
                    .map_err(tool_internal_error)?;

                let mut out = Vec::new();
                if let Some(key) = cluster_key.clone() {
                    let rows = sqlite
                        .list_symbols_in_cluster(&key, limit + 1)
                        .map_err(tool_internal_error)?;
                    for (id, name) in rows {
                        if id == root.id {
                            continue;
                        }
                        if out.len() >= limit {
                            break;
                        }
                        out.push(json!({ "id": id, "name": name }));
                    }
                }

                Ok(CallToolResult::text_content(vec![
                    serde_json::to_string_pretty(&json!({
                        "symbol_name": root.name,
                        "cluster_key": cluster_key,
                        "count": out.len(),
                        "symbols": out,
                    }))
                    .unwrap_or_else(|_| "{\"ok\":true}".to_string())
                    .into(),
                ]))
            }
            "find_references" => {
                let tool: FindReferencesTool = parse_tool_args(&params)?;
                let limit = tool.limit.unwrap_or(200).max(1) as usize;
                let reference_type = tool.reference_type.unwrap_or_else(|| "all".to_string());

                let sqlite =
                    SqliteStore::open(&self.state.config.db_path).map_err(tool_internal_error)?;
                sqlite.init().map_err(tool_internal_error)?;

                let roots = sqlite
                    .search_symbols_by_exact_name(&tool.symbol_name, None, 20)
                    .map_err(tool_internal_error)?;

                let mut out = Vec::new();
                for root in roots {
                    if out.len() >= limit {
                        break;
                    }
                    let edges = sqlite
                        .list_edges_to(&root.id, limit * 3)
                        .map_err(tool_internal_error)?;
                    for e in edges {
                        if out.len() >= limit {
                            break;
                        }
                        if reference_type != "all" && reference_type != e.edge_type {
                            continue;
                        }
                        let from = sqlite
                            .get_symbol_by_id(&e.from_symbol_id)
                            .map_err(tool_internal_error)?;
                        out.push(json!({
                            "to_symbol_id": e.to_symbol_id,
                            "to_symbol_name": root.name,
                            "from_symbol_id": e.from_symbol_id,
                            "from_symbol_name": from.as_ref().map(|s| s.name.clone()).unwrap_or_default(),
                            "reference_type": e.edge_type,
                            "at_file": e.at_file,
                            "at_line": e.at_line,
                        }));
                    }
                }

                Ok(CallToolResult::text_content(vec![
                    serde_json::to_string_pretty(&json!({
                        "symbol_name": tool.symbol_name,
                        "reference_type": reference_type,
                        "count": out.len(),
                        "references": out,
                    }))
                    .unwrap_or_else(|_| "{\"ok\":true}".to_string())
                    .into(),
                ]))
            }
            "get_usage_examples" => {
                let tool: GetUsageExamplesTool = parse_tool_args(&params)?;
                let limit = tool.limit.unwrap_or(20).max(1) as usize;

                let sqlite =
                    SqliteStore::open(&self.state.config.db_path).map_err(tool_internal_error)?;
                sqlite.init().map_err(tool_internal_error)?;

                let roots = sqlite
                    .search_symbols_by_exact_name(&tool.symbol_name, None, 20)
                    .map_err(tool_internal_error)?;

                let mut examples = Vec::new();
                for root in roots {
                    if examples.len() >= limit {
                        break;
                    }
                    let stored = sqlite
                        .list_usage_examples_for_symbol(&root.id, limit * 4)
                        .map_err(tool_internal_error)?;

                    if !stored.is_empty() {
                        for ex in stored {
                            if examples.len() >= limit {
                                break;
                            }
                            let from_symbol_name = ex
                                .from_symbol_id
                                .as_ref()
                                .and_then(|id| sqlite.get_symbol_by_id(id).ok().flatten())
                                .map(|s| s.name)
                                .unwrap_or_default();
                            examples.push(json!({
                                "reference_type": ex.example_type,
                                "from_file_path": ex.file_path,
                                "from_symbol_name": from_symbol_name,
                                "at_file": ex.file_path,
                                "at_line": ex.line,
                                "snippet": ex.snippet,
                            }));
                        }
                        continue;
                    }

                    let edges = sqlite
                        .list_edges_to(&root.id, limit * 4)
                        .map_err(tool_internal_error)?;
                    for e in edges {
                        if examples.len() >= limit {
                            break;
                        }
                        if e.edge_type != "call"
                            && e.edge_type != "import"
                            && e.edge_type != "reference"
                        {
                            continue;
                        }
                        let from = sqlite
                            .get_symbol_by_id(&e.from_symbol_id)
                            .map_err(tool_internal_error)?;
                        let Some(from) = from else {
                            continue;
                        };
                        let snippet =
                            extract_usage_line(&from.text, &root.name).unwrap_or_default();
                        examples.push(json!({
                            "reference_type": e.edge_type,
                            "from_file_path": from.file_path,
                            "from_symbol_name": from.name,
                            "at_file": e.at_file,
                            "at_line": e.at_line,
                            "snippet": snippet,
                        }));
                    }
                }

                Ok(CallToolResult::text_content(vec![
                    serde_json::to_string_pretty(&json!({
                        "symbol_name": tool.symbol_name,
                        "count": examples.len(),
                        "examples": examples,
                    }))
                    .unwrap_or_else(|_| "{\"ok\":true}".to_string())
                    .into(),
                ]))
            }
            "get_call_hierarchy" => {
                let tool: GetCallHierarchyTool = parse_tool_args(&params)?;
                let depth = tool.depth.unwrap_or(2) as usize;
                let limit = tool.limit.unwrap_or(200).max(1) as usize;
                let direction = tool.direction.unwrap_or_else(|| "callees".to_string());

                let sqlite =
                    SqliteStore::open(&self.state.config.db_path).map_err(tool_internal_error)?;
                sqlite.init().map_err(tool_internal_error)?;

                let roots = sqlite
                    .search_symbols_by_exact_name(&tool.symbol_name, None, 10)
                    .map_err(tool_internal_error)?;
                let root = roots.first().cloned();

                let Some(root) = root else {
                    return Ok(CallToolResult::text_content(vec![
                        serde_json::to_string_pretty(&json!({
                            "symbol_name": tool.symbol_name,
                            "direction": direction,
                            "depth": depth,
                            "nodes": [],
                            "edges": [],
                        }))
                        .unwrap_or_else(|_| "{}".to_string())
                        .into(),
                    ]));
                };

                let graph = build_call_hierarchy(&sqlite, &root, &direction, depth, limit)
                    .map_err(tool_internal_error)?;

                Ok(CallToolResult::text_content(vec![
                    serde_json::to_string_pretty(&graph)
                        .unwrap_or_else(|_| "{}".to_string())
                        .into(),
                ]))
            }
            "get_type_graph" => {
                let tool: GetTypeGraphTool = parse_tool_args(&params)?;
                let depth = tool.depth.unwrap_or(2) as usize;
                let limit = tool.limit.unwrap_or(200).max(1) as usize;

                let sqlite =
                    SqliteStore::open(&self.state.config.db_path).map_err(tool_internal_error)?;
                sqlite.init().map_err(tool_internal_error)?;

                let roots = sqlite
                    .search_symbols_by_exact_name(&tool.symbol_name, None, 10)
                    .map_err(tool_internal_error)?;
                let root = roots.first().cloned();

                let Some(root) = root else {
                    return Ok(CallToolResult::text_content(vec![
                        serde_json::to_string_pretty(&json!({
                            "symbol_name": tool.symbol_name,
                            "depth": depth,
                            "nodes": [],
                            "edges": [],
                        }))
                        .unwrap_or_else(|_| "{}".to_string())
                        .into(),
                    ]));
                };

                let graph =
                    build_type_graph(&sqlite, &root, depth, limit).map_err(tool_internal_error)?;
                Ok(CallToolResult::text_content(vec![
                    serde_json::to_string_pretty(&graph)
                        .unwrap_or_else(|_| "{}".to_string())
                        .into(),
                ]))
            }
            _ => Err(CallToolError::unknown_tool(params.name)),
        }
    }
}

fn extract_usage_line(text: &str, symbol_name: &str) -> Option<String> {
    for line in text.lines() {
        if line.contains(symbol_name) {
            let mut s = line.trim().to_string();
            if s.len() > 200 {
                s.truncate(200);
            }
            return Some(s);
        }
    }
    None
}

fn build_dependency_graph(
    sqlite: &SqliteStore,
    root: &SymbolRow,
    direction: &str,
    depth: usize,
    limit: usize,
) -> anyhow::Result<serde_json::Value> {
    let mut nodes = std::collections::HashMap::<String, serde_json::Value>::new();
    let mut edges = Vec::<serde_json::Value>::new();
    let mut visited = std::collections::HashSet::<String>::new();

    // Initial node
    nodes.insert(
        root.id.clone(),
        json!({
            "id": root.id,
            "name": root.name,
            "kind": root.kind,
            "file_path": root.file_path,
            "language": root.language,
            "exported": root.exported,
            "line_range": [root.start_line, root.end_line],
        }),
    );
    visited.insert(root.id.clone());

    let mut frontier = vec![root.id.clone()];

    // Direction flags
    let traverse_upstream = direction == "upstream" || direction == "bidirectional";
    let traverse_downstream = direction == "downstream" || direction == "bidirectional";

    for _ in 0..depth {
        if edges.len() >= limit {
            break;
        }
        let mut next = Vec::new();

        for current_id in frontier {
            if edges.len() >= limit {
                break;
            }

            // Upstream: Who calls me? (Incoming edges)
            if traverse_upstream {
                let incoming = sqlite.list_edges_to(&current_id, limit)?;
                for e in incoming {
                    if edges.len() >= limit {
                        break;
                    }

                    // Filter edge types? "call" is primary. "reference" maybe?
                    if e.edge_type != "call" && e.edge_type != "reference" {
                        continue;
                    }

                    let Some(caller) = sqlite.get_symbol_by_id(&e.from_symbol_id)? else {
                        continue;
                    };

                    // Add node if new
                    if !nodes.contains_key(&caller.id) {
                        nodes.insert(
                            caller.id.clone(),
                            json!({
                                "id": caller.id,
                                "name": caller.name,
                                "kind": caller.kind,
                                "file_path": caller.file_path,
                                "language": caller.language,
                                "exported": caller.exported,
                                "line_range": [caller.start_line, caller.end_line],
                            }),
                        );
                    }

                    // Add edge
                    let evidence = sqlite
                        .list_edge_evidence(&e.from_symbol_id, &e.to_symbol_id, &e.edge_type, 3)
                        .unwrap_or_default();
                    edges.push(json!({
                        "from": e.from_symbol_id,
                        "to": e.to_symbol_id,
                        "edge_type": e.edge_type,
                        "at_file": e.at_file,
                        "at_line": e.at_line,
                        "evidence_count": e.evidence_count,
                        "resolution": e.resolution,
                        "evidence": evidence.into_iter().map(|ev| json!({
                            "at_file": ev.at_file,
                            "at_line": ev.at_line,
                            "count": ev.count,
                        })).collect::<Vec<_>>(),
                    }));

                    if visited.insert(caller.id.clone()) {
                        next.push(caller.id);
                    }
                }
            }

            // Downstream: Who do I call? (Outgoing edges)
            if traverse_downstream {
                let outgoing = sqlite.list_edges_from(&current_id, limit)?;
                for e in outgoing {
                    if edges.len() >= limit {
                        break;
                    }

                    if e.edge_type != "call" && e.edge_type != "reference" {
                        continue;
                    }

                    let Some(callee) = sqlite.get_symbol_by_id(&e.to_symbol_id)? else {
                        continue;
                    };

                    if !nodes.contains_key(&callee.id) {
                        nodes.insert(
                            callee.id.clone(),
                            json!({
                                "id": callee.id,
                                "name": callee.name,
                                "kind": callee.kind,
                                "file_path": callee.file_path,
                                "language": callee.language,
                                "exported": callee.exported,
                                "line_range": [callee.start_line, callee.end_line],
                            }),
                        );
                    }

                    edges.push(json!({
                        "from": e.from_symbol_id,
                        "to": e.to_symbol_id,
                        "edge_type": e.edge_type,
                        "at_file": e.at_file,
                        "at_line": e.at_line,
                        "evidence_count": e.evidence_count,
                        "resolution": e.resolution,
                        "evidence": sqlite
                            .list_edge_evidence(&e.from_symbol_id, &e.to_symbol_id, &e.edge_type, 3)
                            .unwrap_or_default()
                            .into_iter()
                            .map(|ev| json!({
                                "at_file": ev.at_file,
                                "at_line": ev.at_line,
                                "count": ev.count,
                            }))
                            .collect::<Vec<_>>(),
                    }));

                    if visited.insert(callee.id.clone()) {
                        next.push(callee.id);
                    }
                }
            }
        }
        frontier = next;
    }

    Ok(json!({
        "symbol_name": root.name,
        "direction": direction,
        "depth": depth,
        "nodes": nodes.into_values().collect::<Vec<_>>(),
        "edges": edges,
    }))
}

fn build_call_hierarchy(
    sqlite: &SqliteStore,
    root: &SymbolRow,
    direction: &str,
    depth: usize,
    limit: usize,
) -> anyhow::Result<serde_json::Value> {
    let mut nodes = std::collections::HashMap::<String, serde_json::Value>::new();
    let mut edges = Vec::<serde_json::Value>::new();
    let mut visited = std::collections::HashSet::<String>::new();

    nodes.insert(
        root.id.clone(),
        json!({
            "id": root.id,
            "name": root.name,
            "kind": root.kind,
            "file_path": root.file_path,
            "language": root.language,
            "exported": root.exported,
            "line_range": [root.start_line, root.end_line],
        }),
    );
    visited.insert(root.id.clone());

    let mut frontier = vec![root.id.clone()];
    for _ in 0..depth {
        if edges.len() >= limit {
            break;
        }
        let mut next = Vec::new();
        for current_id in frontier {
            if edges.len() >= limit {
                break;
            }
            if direction == "callers" {
                let incoming = sqlite.list_edges_to(&current_id, limit)?;
                for e in incoming {
                    if edges.len() >= limit {
                        break;
                    }
                    if e.edge_type != "call" {
                        continue;
                    }
                    let Some(caller) = sqlite.get_symbol_by_id(&e.from_symbol_id)? else {
                        continue;
                    };
                    nodes.entry(caller.id.clone()).or_insert_with(|| {
                        json!({
                            "id": caller.id,
                            "name": caller.name,
                            "kind": caller.kind,
                            "file_path": caller.file_path,
                            "language": caller.language,
                            "exported": caller.exported,
                            "line_range": [caller.start_line, caller.end_line],
                        })
                    });
                    edges.push(json!({
                        "from": e.from_symbol_id,
                        "to": e.to_symbol_id,
                        "edge_type": "call",
                        "at_file": e.at_file,
                        "at_line": e.at_line,
                        "evidence_count": e.evidence_count,
                        "resolution": e.resolution,
                        "evidence": sqlite
                            .list_edge_evidence(&e.from_symbol_id, &e.to_symbol_id, "call", 3)
                            .unwrap_or_default()
                            .into_iter()
                            .map(|ev| json!({
                                "at_file": ev.at_file,
                                "at_line": ev.at_line,
                                "count": ev.count,
                            }))
                            .collect::<Vec<_>>(),
                    }));
                    if visited.insert(caller.id.clone()) {
                        next.push(caller.id);
                    }
                }
            } else {
                let outgoing = sqlite.list_edges_from(&current_id, limit)?;
                for e in outgoing {
                    if edges.len() >= limit {
                        break;
                    }
                    if e.edge_type != "call" {
                        continue;
                    }
                    let Some(callee) = sqlite.get_symbol_by_id(&e.to_symbol_id)? else {
                        continue;
                    };
                    nodes.entry(callee.id.clone()).or_insert_with(|| {
                        json!({
                            "id": callee.id,
                            "name": callee.name,
                            "kind": callee.kind,
                            "file_path": callee.file_path,
                            "language": callee.language,
                            "exported": callee.exported,
                            "line_range": [callee.start_line, callee.end_line],
                        })
                    });
                    edges.push(json!({
                        "from": e.from_symbol_id,
                        "to": e.to_symbol_id,
                        "edge_type": "call",
                        "at_file": e.at_file,
                        "at_line": e.at_line,
                        "evidence_count": e.evidence_count,
                        "resolution": e.resolution,
                        "evidence": sqlite
                            .list_edge_evidence(&e.from_symbol_id, &e.to_symbol_id, "call", 3)
                            .unwrap_or_default()
                            .into_iter()
                            .map(|ev| json!({
                                "at_file": ev.at_file,
                                "at_line": ev.at_line,
                                "count": ev.count,
                            }))
                            .collect::<Vec<_>>(),
                    }));
                    if visited.insert(callee.id.clone()) {
                        next.push(callee.id);
                    }
                }
            }
        }
        frontier = next;
    }

    Ok(json!({
        "symbol_name": root.name,
        "direction": direction,
        "depth": depth,
        "nodes": nodes.into_values().collect::<Vec<_>>(),
        "edges": edges,
    }))
}

fn build_type_graph(
    sqlite: &SqliteStore,
    root: &SymbolRow,
    depth: usize,
    limit: usize,
) -> anyhow::Result<serde_json::Value> {
    let mut nodes = std::collections::HashMap::<String, serde_json::Value>::new();
    let mut edges = Vec::<serde_json::Value>::new();
    let mut visited = std::collections::HashSet::<String>::new();

    nodes.insert(
        root.id.clone(),
        json!({
            "id": root.id,
            "name": root.name,
            "kind": root.kind,
            "file_path": root.file_path,
            "language": root.language,
            "exported": root.exported,
            "line_range": [root.start_line, root.end_line],
        }),
    );
    visited.insert(root.id.clone());

    let mut frontier = vec![root.id.clone()];
    for _ in 0..depth {
        if edges.len() >= limit {
            break;
        }
        let mut next = Vec::new();
        for current_id in frontier {
            if edges.len() >= limit {
                break;
            }
            let outgoing = sqlite.list_edges_from(&current_id, limit)?;
            for e in outgoing {
                if edges.len() >= limit {
                    break;
                }
                if e.edge_type != "extends" && e.edge_type != "implements" && e.edge_type != "alias"
                {
                    continue;
                }
                let Some(to_sym) = sqlite.get_symbol_by_id(&e.to_symbol_id)? else {
                    continue;
                };
                nodes.entry(to_sym.id.clone()).or_insert_with(|| {
                    json!({
                        "id": to_sym.id,
                        "name": to_sym.name,
                        "kind": to_sym.kind,
                        "file_path": to_sym.file_path,
                        "language": to_sym.language,
                        "exported": to_sym.exported,
                        "line_range": [to_sym.start_line, to_sym.end_line],
                    })
                });
                edges.push(json!({
                    "from": e.from_symbol_id,
                    "to": e.to_symbol_id,
                    "edge_type": e.edge_type,
                    "at_file": e.at_file,
                    "at_line": e.at_line,
                    "evidence_count": e.evidence_count,
                    "resolution": e.resolution,
                    "evidence": sqlite
                        .list_edge_evidence(&e.from_symbol_id, &e.to_symbol_id, &e.edge_type, 3)
                        .unwrap_or_default()
                        .into_iter()
                        .map(|ev| json!({
                            "at_file": ev.at_file,
                            "at_line": ev.at_line,
                            "count": ev.count,
                        }))
                        .collect::<Vec<_>>(),
                }));
                if visited.insert(to_sym.id.clone()) {
                    next.push(to_sym.id);
                }
            }
        }
        frontier = next;
    }

    Ok(json!({
        "symbol_name": root.name,
        "depth": depth,
        "nodes": nodes.into_values().collect::<Vec<_>>(),
        "edges": edges,
    }))
}

#[cfg(feature = "web-ui")]
fn env_true(key: &str) -> bool {
    std::env::var(key)
        .ok()
        .map(|v| matches!(v.trim().to_lowercase().as_str(), "1" | "true" | "yes" | "y"))
        .unwrap_or(false)
}

#[cfg(feature = "web-ui")]
mod web_ui {
    use super::{tool_internal_error, AppState};
    use axum::{
        extract::{Query, State},
        response::{Html, IntoResponse},
        routing::get,
        Json, Router,
    };
    use serde::Deserialize;
    use serde_json::json;
    use std::{net::SocketAddr, sync::Arc};

    #[derive(Debug, Deserialize)]
    pub struct SearchParams {
        pub query: String,
        pub limit: Option<usize>,
        pub exported_only: Option<bool>,
    }

    #[derive(Debug, Deserialize)]
    pub struct NameParam {
        pub name: String,
    }

    pub async fn spawn(state: Arc<AppState>) -> anyhow::Result<()> {
        let addr: SocketAddr = std::env::var("WEB_UI_ADDR")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or_else(|| SocketAddr::from(([127, 0, 0, 1], 8787)));

        let app = Router::new()
            .route("/", get(index))
            .route("/api/search", get(api_search))
            .route("/api/definition", get(api_definition))
            .route("/api/edges", get(api_edges))
            .with_state(state);

        let listener = tokio::net::TcpListener::bind(addr).await?;
        tokio::spawn(async move {
            let _ = axum::serve(listener, app.into_make_service()).await;
        });
        Ok(())
    }

    async fn index() -> Html<&'static str> {
        Html(
            r#"<!doctype html>
<html>
  <head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <title>Code Intelligence</title>
    <style>
      body { font-family: ui-sans-serif, system-ui, -apple-system, sans-serif; margin: 24px; }
      input, button { font-size: 14px; padding: 8px; }
      pre { white-space: pre-wrap; background: #f6f8fa; padding: 12px; border-radius: 6px; }
      .row { display: flex; gap: 8px; align-items: center; margin-bottom: 12px; }
      .col { margin-top: 12px; }
    </style>
  </head>
  <body>
    <h1>Code Intelligence</h1>
    <div class="row">
      <input id="q" size="60" placeholder="search query or symbol name" />
      <button onclick="runSearch()">Search</button>
      <button onclick="runDef()">Definition</button>
      <button onclick="runEdges()">Edges</button>
    </div>
    <div class="col"><pre id="out"></pre></div>
    <script>
      async function getJson(url) {
        const res = await fetch(url);
        const txt = await res.text();
        try { return JSON.stringify(JSON.parse(txt), null, 2); }
        catch { return txt; }
      }
      async function runSearch() {
        const q = encodeURIComponent(document.getElementById('q').value);
        document.getElementById('out').textContent = await getJson(`/api/search?query=${q}`);
      }
      async function runDef() {
        const name = encodeURIComponent(document.getElementById('q').value);
        document.getElementById('out').textContent = await getJson(`/api/definition?name=${name}`);
      }
      async function runEdges() {
        const name = encodeURIComponent(document.getElementById('q').value);
        document.getElementById('out').textContent = await getJson(`/api/edges?name=${name}`);
      }
    </script>
  </body>
</html>
"#,
        )
    }

    async fn api_search(
        State(state): State<Arc<AppState>>,
        Query(params): Query<SearchParams>,
    ) -> impl IntoResponse {
        let limit = params.limit.unwrap_or(5).max(1);
        let exported_only = params.exported_only.unwrap_or(false);
        match state
            .retriever
            .search(&params.query, limit, exported_only)
            .await
        {
            Ok(resp) => Json(resp).into_response(),
            Err(err) => {
                Json(json!({ "error": tool_internal_error(err).to_string() })).into_response()
            }
        }
    }

    async fn api_definition(
        State(state): State<Arc<AppState>>,
        Query(params): Query<NameParam>,
    ) -> impl IntoResponse {
        let sqlite = match super::SqliteStore::open(&state.config.db_path) {
            Ok(s) => s,
            Err(err) => return Json(json!({ "error": err.to_string() })).into_response(),
        };
        if let Err(err) = sqlite.init() {
            return Json(json!({ "error": err.to_string() })).into_response();
        }
        let rows = match sqlite.search_symbols_by_exact_name(&params.name, None, 10) {
            Ok(r) => r,
            Err(err) => return Json(json!({ "error": err.to_string() })).into_response(),
        };
        let context = state
            .retriever
            .assemble_definitions(&rows)
            .unwrap_or_default();
        Json(json!({ "symbol_name": params.name, "count": rows.len(), "definitions": rows, "context": context }))
            .into_response()
    }

    async fn api_edges(
        State(state): State<Arc<AppState>>,
        Query(params): Query<NameParam>,
    ) -> impl IntoResponse {
        let sqlite = match super::SqliteStore::open(&state.config.db_path) {
            Ok(s) => s,
            Err(err) => return Json(json!({ "error": err.to_string() })).into_response(),
        };
        if let Err(err) = sqlite.init() {
            return Json(json!({ "error": err.to_string() })).into_response();
        }
        let roots = match sqlite.search_symbols_by_exact_name(&params.name, None, 10) {
            Ok(r) => r,
            Err(err) => return Json(json!({ "error": err.to_string() })).into_response(),
        };
        let root = match roots.first() {
            Some(r) => r,
            None => {
                return Json(json!({ "symbol_name": params.name, "edges": [] })).into_response()
            }
        };
        let edges = sqlite.list_edges_from(&root.id, 500).unwrap_or_default();
        Json(json!({ "symbol_name": root.name, "edges": edges })).into_response()
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use axum::response::IntoResponse;
        use code_intelligence_mcp_server::{
            config::{EmbeddingsBackend, EmbeddingsDevice},
            embeddings::hash::HashEmbedder,
            indexer::pipeline::IndexPipeline,
            retrieval::Retriever,
            storage::{tantivy::TantivyIndex, vector::LanceDbStore},
        };
        use http_body_util::BodyExt;
        use serde_json::Value;
        use std::{
            path::{Path, PathBuf},
            sync::Arc,
        };
        use tokio::sync::Mutex;

        fn tmp_dir() -> PathBuf {
            let dir = std::env::temp_dir().join(format!(
                "code-intel-webui-test-{}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir_all(&dir).unwrap();
            dir
        }

        fn cfg(base: &Path) -> super::super::Config {
            let base = base.canonicalize().unwrap_or_else(|_| base.to_path_buf());
            super::super::Config {
                base_dir: base.clone(),
                db_path: base.join("code-intelligence.db"),
                vector_db_path: base.join("vectors"),
                tantivy_index_path: base.join("tantivy-index"),
                embeddings_backend: EmbeddingsBackend::Hash,
                embeddings_model_dir: None,
                embeddings_model_url: None,
                embeddings_model_sha256: None,
                embeddings_auto_download: false,
                embeddings_model_repo: None,
                embeddings_model_revision: None,
                embeddings_model_hf_token: None,
                embeddings_device: EmbeddingsDevice::Cpu,
                embedding_batch_size: 32,
                hash_embedding_dim: 32,
                vector_search_limit: 20,
                hybrid_alpha: 0.7,
                rank_vector_weight: 0.7,
                rank_keyword_weight: 0.3,
                rank_exported_boost: 0.1,
                rank_index_file_boost: 0.05,
                rank_test_penalty: 0.1,
                rank_popularity_weight: 0.05,
                rank_popularity_cap: 50,
                index_patterns: vec![],
                exclude_patterns: vec![],
                watch_mode: false,
                watch_debounce_ms: 50,
                max_context_bytes: 200_000,
                index_node_modules: false,
                repo_roots: vec![base],
            }
        }

        async fn build_state(base: &Path) -> Arc<AppState> {
            let mut config = cfg(base);

            let src = base.join("src");
            std::fs::create_dir_all(&src).unwrap();
            std::fs::write(
                src.join("a.ts"),
                r#"
export function alpha() { return 1 }
export function beta() { return alpha() }
"#,
            )
            .unwrap();

            config.repo_roots = vec![config.base_dir.clone()];
            let config = Arc::new(config);

            let tantivy =
                Arc::new(TantivyIndex::open_or_create(&config.tantivy_index_path).unwrap());
            let lancedb = LanceDbStore::connect(&config.vector_db_path).await.unwrap();
            let vectors = Arc::new(
                lancedb
                    .open_or_create_table("symbols", config.hash_embedding_dim)
                    .await
                    .unwrap(),
            );
            let embedder = Arc::new(Mutex::new(Box::new(HashEmbedder::new(
                config.hash_embedding_dim,
            )) as _));

            let indexer = IndexPipeline::new(
                config.clone(),
                tantivy.clone(),
                vectors.clone(),
                embedder.clone(),
            );
            let retriever = Retriever::new(config.clone(), tantivy, vectors, embedder);

            indexer.index_all().await.unwrap();

            Arc::new(AppState {
                config,
                indexer,
                retriever,
            })
        }

        async fn body_json(resp: axum::response::Response) -> Value {
            let bytes = resp.into_body().collect().await.unwrap().to_bytes();
            serde_json::from_slice(&bytes).unwrap()
        }

        #[tokio::test]
        async fn index_page_serves_html() {
            let Html(body) = index().await;
            assert!(body.contains("<title>Code Intelligence</title>"));
        }

        #[tokio::test]
        async fn api_search_returns_json_and_context() {
            let base = tmp_dir();
            let state = build_state(&base).await;

            let resp = api_search(
                State(state),
                Query(SearchParams {
                    query: "beta".to_string(),
                    limit: Some(5),
                    exported_only: Some(false),
                }),
            )
            .await
            .into_response();

            assert!(resp.status().is_success());
            let v = body_json(resp).await;
            assert_eq!(v.get("query").and_then(|x| x.as_str()), Some("beta"));
            assert!(v
                .get("context")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .contains("export function beta"));
        }

        #[tokio::test]
        async fn api_definition_returns_rows_and_context() {
            let base = tmp_dir();
            let state = build_state(&base).await;

            let resp = api_definition(
                State(state),
                Query(NameParam {
                    name: "beta".to_string(),
                }),
            )
            .await
            .into_response();

            assert!(resp.status().is_success());
            let v = body_json(resp).await;
            assert!(v.get("count").and_then(|x| x.as_u64()).unwrap_or(0) >= 1);
            assert!(v
                .get("context")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .contains("export function beta"));
        }

        #[tokio::test]
        async fn api_edges_returns_edges_and_handles_missing_symbol() {
            let base = tmp_dir();
            let state = build_state(&base).await;

            let resp = api_edges(
                State(state.clone()),
                Query(NameParam {
                    name: "beta".to_string(),
                }),
            )
            .await
            .into_response();
            assert!(resp.status().is_success());
            let v = body_json(resp).await;
            let edges = v
                .get("edges")
                .and_then(|x| x.as_array())
                .cloned()
                .unwrap_or_default();
            assert!(!edges.is_empty());
            assert!(edges
                .iter()
                .any(|e| e.get("edge_type").and_then(|x| x.as_str()) == Some("call")));

            let resp2 = api_edges(
                State(state),
                Query(NameParam {
                    name: "does_not_exist".to_string(),
                }),
            )
            .await
            .into_response();
            assert!(resp2.status().is_success());
            let v2 = body_json(resp2).await;
            let edges2 = v2
                .get("edges")
                .and_then(|x| x.as_array())
                .cloned()
                .unwrap_or_default();
            assert!(edges2.is_empty());
        }
    }
}

#[tokio::main]
async fn main() -> SdkResult<()> {
    let args = std::env::args().collect::<Vec<_>>();
    if wants_help(&args) {
        print_help();
        return Ok(());
    }
    if wants_version(&args) {
        print_version();
        return Ok(());
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();

    info!(
        version = env!("CARGO_PKG_VERSION"),
        "Starting code-intelligence-mcp-server"
    );

    if let Err(err) = run().await {
        error!(error = %err, "Server exited with error");
        return Err(err);
    }
    Ok(())
}

async fn run() -> SdkResult<()> {
    let config = Config::from_env().map_err(|err| McpSdkError::Internal {
        description: err.to_string(),
    })?;

    let sqlite = SqliteStore::open(&config.db_path).map_err(|err| McpSdkError::Internal {
        description: err.to_string(),
    })?;
    sqlite.init().map_err(|err| McpSdkError::Internal {
        description: err.to_string(),
    })?;

    let embedder: Box<dyn Embedder + Send> = match config.embeddings_backend {
        EmbeddingsBackend::FastEmbed => {
            let model_repo = config
                .embeddings_model_repo
                .as_deref()
                .unwrap_or("BAAI/bge-base-en-v1.5");
            let cache_dir = config.embeddings_model_dir.as_deref();

            info!(
                "Initializing FastEmbed with model: {} (cache: {:?})",
                model_repo, cache_dir
            );

            Box::new(
                code_intelligence_mcp_server::embeddings::fastembed::FastEmbedder::new(
                    model_repo,
                    cache_dir,
                    config.embeddings_device,
                )
                .map_err(|err| McpSdkError::Internal {
                    description: format!("Failed to initialize FastEmbed: {}", err),
                })?,
            )
        }
        EmbeddingsBackend::Hash => Box::new(HashEmbedder::new(config.hash_embedding_dim)),
    };

    let tantivy = TantivyIndex::open_or_create(&config.tantivy_index_path).map_err(|err| {
        McpSdkError::Internal {
            description: err.to_string(),
        }
    })?;

    let vector_dim = embedder.dim();
    let embedder: Arc<Mutex<Box<dyn Embedder + Send>>> = Arc::new(Mutex::new(embedder));

    let lancedb = LanceDbStore::connect(&config.vector_db_path)
        .await
        .map_err(|err| McpSdkError::Internal {
            description: err.to_string(),
        })?;
    let vectors = lancedb
        .open_or_create_table("symbols", vector_dim)
        .await
        .map_err(|err| McpSdkError::Internal {
            description: err.to_string(),
        })?;

    let config = Arc::new(config);
    let tantivy = Arc::new(tantivy);
    let vectors = Arc::new(vectors);

    let indexer = IndexPipeline::new(
        config.clone(),
        tantivy.clone(),
        vectors.clone(),
        embedder.clone(),
    );
    let retriever = Retriever::new(
        config.clone(),
        tantivy.clone(),
        vectors.clone(),
        embedder.clone(),
    );

    let state = Arc::new(AppState {
        config: config.clone(),
        indexer,
        retriever,
    });

    if state.config.watch_mode {
        state.indexer.spawn_watch_loop();
    }

    #[cfg(feature = "web-ui")]
    if env_true("WEB_UI") {
        web_ui::spawn(state.clone())
            .await
            .map_err(|err| McpSdkError::Internal {
                description: err.to_string(),
            })?;
    }

    let db_path_rel = config.path_relative_to_base(&config.db_path).ok();
    let vector_db_path_rel = config.path_relative_to_base(&config.vector_db_path).ok();
    let tantivy_index_path_rel = config
        .path_relative_to_base(&config.tantivy_index_path)
        .ok();

    debug!(
        base_dir = %config.base_dir.display(),
        db_path = %config.db_path.display(),
        db_path_rel = ?db_path_rel,
        vector_db_path = %config.vector_db_path.display(),
        vector_db_path_rel = ?vector_db_path_rel,
        tantivy_index_path = %config.tantivy_index_path.display(),
        tantivy_index_path_rel = ?tantivy_index_path_rel,
        embeddings_backend = ?config.embeddings_backend,
        embeddings_model_dir = ?config.embeddings_model_dir.as_ref().map(|p| p.display().to_string()),
        embeddings_device = ?config.embeddings_device,
        embedding_batch_size = config.embedding_batch_size,
        hash_embedding_dim = config.hash_embedding_dim,
        vector_search_limit = config.vector_search_limit,
        hybrid_alpha = config.hybrid_alpha,
        rank_vector_weight = config.rank_vector_weight,
        rank_keyword_weight = config.rank_keyword_weight,
        rank_exported_boost = config.rank_exported_boost,
        rank_index_file_boost = config.rank_index_file_boost,
        rank_test_penalty = config.rank_test_penalty,
        rank_popularity_weight = config.rank_popularity_weight,
        rank_popularity_cap = config.rank_popularity_cap,
        watch_mode = config.watch_mode,
        watch_debounce_ms = config.watch_debounce_ms,
        max_context_bytes = config.max_context_bytes,
        index_node_modules = config.index_node_modules,
        repo_roots = ?config.repo_roots,
        "Loaded config"
    );

    info!(
        embeddings_backend = ?config.embeddings_backend,
        watch_mode = config.watch_mode,
        "Initialized components"
    );

    let server_details = InitializeResult {
        server_info: Implementation {
            name: "code-intelligence".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            title: Some("Code Intelligence MCP".into()),
            description: Some("Local code intelligence MCP server".into()),
            icons: vec![],
            website_url: None,
        },
        capabilities: ServerCapabilities {
            tools: Some(ServerCapabilitiesTools { list_changed: None }),
            ..Default::default()
        },
        protocol_version: ProtocolVersion::V2025_11_25.into(),
        instructions: None,
        meta: None,
    };

    let transport = StdioTransport::new(TransportOptions::default())?;
    let handler = CodeIntelligenceHandler { state }.to_mcp_server_handler();

    let server = server_runtime::create_server(McpServerOptions {
        server_details,
        transport,
        handler,
        task_store: None,
        client_task_store: None,
    });

    info!("Starting MCP stdio server");
    server.start().await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sym(id: &str, name: &str) -> SymbolRow {
        SymbolRow {
            id: id.to_string(),
            file_path: "src/a.ts".to_string(),
            language: "typescript".to_string(),
            kind: "function".to_string(),
            name: name.to_string(),
            exported: true,
            start_byte: 0,
            end_byte: 1,
            start_line: 1,
            end_line: 1,
            text: format!("export function {name}() {{}}"),
        }
    }

    #[test]
    fn usage_line_extracts_and_trims() {
        let text = "line1\n   call alpha();   \nline3";
        let got = extract_usage_line(text, "alpha").unwrap();
        assert_eq!(got, "call alpha();");
    }

    #[test]
    fn call_hierarchy_traverses_callees_and_callers() {
        let sqlite = SqliteStore::from_connection(rusqlite::Connection::open_in_memory().unwrap());
        sqlite.init().unwrap();

        let a = sym("a", "alpha");
        let b = sym("b", "beta");
        let c = sym("c", "gamma");
        sqlite.upsert_symbol(&a).unwrap();
        sqlite.upsert_symbol(&b).unwrap();
        sqlite.upsert_symbol(&c).unwrap();

        sqlite
            .upsert_edge(&code_intelligence_mcp_server::storage::sqlite::EdgeRow {
                from_symbol_id: "a".to_string(),
                to_symbol_id: "b".to_string(),
                edge_type: "call".to_string(),
                at_file: Some("src/a.ts".to_string()),
                at_line: Some(1),
                confidence: 1.0,
                evidence_count: 1,
                resolution: "local".to_string(),
            })
            .unwrap();
        sqlite
            .upsert_edge(&code_intelligence_mcp_server::storage::sqlite::EdgeRow {
                from_symbol_id: "b".to_string(),
                to_symbol_id: "c".to_string(),
                edge_type: "call".to_string(),
                at_file: Some("src/a.ts".to_string()),
                at_line: Some(1),
                confidence: 1.0,
                evidence_count: 1,
                resolution: "local".to_string(),
            })
            .unwrap();

        let g1 = build_call_hierarchy(&sqlite, &a, "callees", 3, 100).unwrap();
        let nodes1 = g1.get("nodes").unwrap().as_array().unwrap();
        let edges1 = g1.get("edges").unwrap().as_array().unwrap();
        assert_eq!(edges1.len(), 2);
        assert_eq!(nodes1.len(), 3);

        let g2 = build_call_hierarchy(&sqlite, &c, "callers", 3, 100).unwrap();
        let nodes2 = g2.get("nodes").unwrap().as_array().unwrap();
        let edges2 = g2.get("edges").unwrap().as_array().unwrap();
        assert_eq!(edges2.len(), 2);
        assert_eq!(nodes2.len(), 3);
    }

    #[test]
    fn type_graph_follows_extends_implements_and_alias() {
        let sqlite = SqliteStore::from_connection(rusqlite::Connection::open_in_memory().unwrap());
        sqlite.init().unwrap();

        let a = sym("a", "A");
        let b = sym("b", "B");
        let c = sym("c", "C");
        let d = sym("d", "D");
        sqlite.upsert_symbol(&a).unwrap();
        sqlite.upsert_symbol(&b).unwrap();
        sqlite.upsert_symbol(&c).unwrap();
        sqlite.upsert_symbol(&d).unwrap();

        for (from, to, ty) in [
            ("a", "b", "extends"),
            ("b", "c", "implements"),
            ("c", "d", "alias"),
        ] {
            sqlite
                .upsert_edge(&code_intelligence_mcp_server::storage::sqlite::EdgeRow {
                    from_symbol_id: from.to_string(),
                    to_symbol_id: to.to_string(),
                    edge_type: ty.to_string(),
                    at_file: Some("src/a.ts".to_string()),
                    at_line: Some(1),
                    confidence: 1.0,
                    evidence_count: 1,
                    resolution: "local".to_string(),
                })
                .unwrap();
        }

        let g = build_type_graph(&sqlite, &a, 3, 100).unwrap();
        let nodes = g.get("nodes").unwrap().as_array().unwrap();
        let edges = g.get("edges").unwrap().as_array().unwrap();
        assert_eq!(nodes.len(), 4);
        assert_eq!(edges.len(), 3);
    }

    #[test]
    fn wants_help_and_version_detect_common_flags() {
        assert!(wants_help(&["bin".to_string(), "--help".to_string()]));
        assert!(wants_help(&["bin".to_string(), "-h".to_string()]));
        assert!(wants_version(&["bin".to_string(), "--version".to_string()]));
        assert!(wants_version(&["bin".to_string(), "-V".to_string()]));
        assert!(!wants_help(&["bin".to_string()]));
        assert!(!wants_version(&["bin".to_string()]));
    }

    #[cfg(feature = "web-ui")]
    #[test]
    fn env_true_accepts_multiple_spellings() {
        std::env::remove_var("X");
        assert!(!env_true("X"));
        std::env::set_var("X", "true");
        assert!(env_true("X"));
        std::env::set_var("X", "1");
        assert!(env_true("X"));
        std::env::set_var("X", "yes");
        assert!(env_true("X"));
        std::env::set_var("X", "y");
        assert!(env_true("X"));
        std::env::set_var("X", "false");
        assert!(!env_true("X"));
        std::env::remove_var("X");
    }
}
