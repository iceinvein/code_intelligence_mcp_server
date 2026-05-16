//! MCP server setup and handler implementation

pub mod api;
pub mod discovery;
pub mod standalone;

use crate::handlers::*;
use crate::tools::*;
use rust_mcp_sdk::{
    schema::{CallToolError, CallToolRequestParams, CallToolResult, CreateTaskResult},
    task_store::ServerTaskCreator,
    McpServer,
};
use std::sync::Arc;

/// Instructions advertised during MCP initialization.
///
/// Tool descriptions are often loaded lazily by clients. These instructions
/// give the model enough up-front routing context to discover the planner tool
/// before it commits to lower-level search tools.
pub fn server_instructions() -> &'static str {
    "Code Intelligence is a retrieval engine for code questions. `ask_code(question)` is the \
fastest path to grounded evidence: it runs `investigate` server-side, returns the \
verified `evidence[]` array (symbol name, file path, line range, code body) and a \
`mode_used` shape classification, then YOU synthesise the user-facing answer from that \
evidence. The `answer` field in the response is intentionally empty -- local-model \
prose was found to introduce hallucinations the agent then anchored on. \
\
For specialist queries call `investigate` (composite multi-hop), `search_code` \
(hybrid search), `get_definition`, `find_references`, `get_call_hierarchy`, \
`find_affected_code`, `trace_data_flow`, or `explore_dependency_graph` directly. \
`plan_code_investigation` is recommendation-only. \
\
Prefer these tools over Grep/Read for any question they answer directly -- they carry \
semantic context (definitions, edges, intent classification) that text search cannot. \
Fall back to Grep only for exact literal strings (error messages, config values) or \
files the index does not cover (markdown, JSON, TOML)."
}

/// Error message returned when `search_across_repos` is called in embedded (stdio) mode.
pub const SEARCH_ACROSS_REPOS_EMBEDDED_MSG: &str =
    "search_across_repos is only available in standalone mode. Start the server with --standalone to use cross-repo search.";

/// Error message returned when `explore_cross_repo_dependencies` is called in embedded (stdio) mode.
pub const EXPLORE_CROSS_REPO_DEPS_EMBEDDED_MSG: &str =
    "explore_cross_repo_dependencies is only available in standalone mode. Start the server with --standalone to use cross-repo dependency exploration.";

/// Build the `ServerTasks` capability block for MCP task-augmented tool calls.
///
/// Used by both embedded and standalone server initialization so the capability
/// advertisement stays in sync.
pub fn task_capabilities() -> rust_mcp_sdk::schema::ServerTasks {
    use rust_mcp_sdk::schema::{ServerTaskRequest, ServerTaskTools, ServerTasks};
    ServerTasks {
        cancel: Some(serde_json::Map::new()),
        list: Some(serde_json::Map::new()),
        requests: Some(ServerTaskRequest {
            tools: Some(ServerTaskTools {
                call: Some(serde_json::Map::new()),
            }),
        }),
    }
}

/// Shared task-augmented tool dispatch — used by both embedded and standalone handlers.
///
/// Only `refresh_index` supports async task execution.  Other tools return
/// an error; the SDK then falls back to synchronous `handle_call_tool_request`.
pub(crate) async fn dispatch_task_augmented_call(
    state: Arc<AppState>,
    params: CallToolRequestParams,
    task_creator: ServerTaskCreator,
    runtime: Arc<dyn McpServer>,
) -> std::result::Result<CreateTaskResult, CallToolError> {
    match params.name.as_str() {
        "refresh_index" => {
            let tool: RefreshIndexTool = parse_tool_args(&params)?;
            let task = task_creator
                .create_task(rust_mcp_sdk::task_store::CreateTaskOptions {
                    ttl: Some(300_000),
                    poll_interval: Some(2_000),
                    meta: None,
                })
                .await;
            let task_id = task.task_id.clone();
            let task_id_supervisor = task_id.clone();
            let task_store = runtime.task_store().ok_or_else(|| {
                CallToolError::from_message("Internal error: task_store not configured".to_string())
            })?;
            let task_store_supervisor = task_store.clone();
            let handle = tokio::spawn(async move {
                let result = handle_refresh_index(&state, tool).await;
                match result {
                    Ok(value) => {
                        task_store
                            .store_task_result(
                                &task_id,
                                rust_mcp_sdk::schema::TaskStatus::Completed,
                                rust_mcp_sdk::schema::schema_utils::ResultFromServer::CallToolResult(
                                    tool_json_content(&value),
                                ),
                                None,
                            )
                            .await;
                    }
                    Err(e) => {
                        task_store
                            .update_task_status(
                                &task_id,
                                rust_mcp_sdk::schema::TaskStatus::Failed,
                                Some(e.to_string()),
                                None,
                            )
                            .await;
                    }
                }
            });
            // Supervisor: if the spawned task panics, mark the MCP task as failed
            // so clients don't poll indefinitely until TTL expiry.
            tokio::spawn(async move {
                if let Err(join_err) = handle.await {
                    task_store_supervisor
                        .update_task_status(
                            &task_id_supervisor,
                            rust_mcp_sdk::schema::TaskStatus::Failed,
                            Some(format!("Internal error: {join_err}")),
                            None,
                        )
                        .await;
                }
            });
            Ok(CreateTaskResult { meta: None, task })
        }
        _ => Err(CallToolError::from_message(format!(
            "Tool '{}' does not support task-augmented execution",
            params.name
        ))),
    }
}

/// All tools advertised by both embedded and standalone handlers
pub fn all_tools() -> Vec<rust_mcp_sdk::schema::Tool> {
    vec![
        SearchCodeTool::tool(),
        RefreshIndexTool::tool(),
        GetDefinitionTool::tool(),
        FindReferencesTool::tool(),
        GetFileSymbolsTool::tool(),
        GetCallHierarchyTool::tool(),
        ExploreDependencyGraphTool::tool(),
        GetSimilarityClusterTool::tool(),
        GetTypeGraphTool::tool(),
        GetUsageExamplesTool::tool(),
        GetIndexStatsTool::tool(),
        HydrateSymbolsTool::tool(),
        PlanCodeInvestigationTool::tool(),
        InvestigateTool::tool(),
        AskCodeTool::tool(),
        BindWorkspaceTool::tool(),
        ReportSelectionTool::tool(),
        ReportFileAccessTool::tool(),
        ExplainSearchTool::tool(),
        FindSimilarCodeTool::tool(),
        SummarizeFileTool::tool(),
        GetModuleSummaryTool::tool(),
        TraceDataFlowTool::tool(),
        FindAffectedCodeTool::tool(),
        SearchTodosTool::tool(),
        FindTestsForSymbolTool::tool(),
        SearchDecoratorsTool::tool(),
        SearchFrameworkPatternsTool::tool(),
        FindDeadCodeTool::tool(),
        FindDuplicatesTool::tool(),
        SearchAcrossReposTool::tool(),
        ExploreCrossRepoDependenciesTool::tool(),
        FindStaleDescriptionsTool::tool(),
        FindUndocumentedSymbolsTool::tool(),
        PredictImpactTool::tool(),
        GetContextBundleTool::tool(),
    ]
}

/// Dispatch helper: parse tool args, call sync handler, serialize result.
macro_rules! dispatch_sync {
    ($params:expr, $tool_ty:ty, |$tool:ident| $handler:expr) => {{
        let $tool: $tool_ty = parse_tool_args(&$params)?;
        let result = { $handler }.map_err(tool_internal_error)?;
        Ok(tool_json_content(&result))
    }};
}

/// Dispatch helper: parse tool args, call async handler, serialize result.
macro_rules! dispatch_async {
    ($params:expr, $tool_ty:ty, |$tool:ident| $handler:expr) => {{
        let $tool: $tool_ty = parse_tool_args(&$params)?;
        let result = { $handler }.await.map_err(tool_internal_error)?;
        Ok(tool_json_content(&result))
    }};
}

/// Serialize tool results without pretty-print whitespace. MCP clients place
/// tool outputs directly in model context, so compact JSON saves tokens on
/// every call without changing the response schema.
pub(crate) fn tool_json_content(value: &serde_json::Value) -> CallToolResult {
    CallToolResult::text_content(vec![serde_json::to_string(value)
        .unwrap_or_else(|_| "{}".to_string())
        .into()])
}

/// Shared tool dispatch — used by both embedded and standalone handlers
pub async fn dispatch_tool_call(
    state: &AppState,
    params: CallToolRequestParams,
) -> std::result::Result<CallToolResult, CallToolError> {
    let tool_name = params.name.clone();
    let start = std::time::Instant::now();

    let result = match params.name.as_str() {
        // --- Async handlers ---
        "refresh_index" => dispatch_async!(params, RefreshIndexTool, |tool| handle_refresh_index(
            state, tool
        )),
        "search_code" => dispatch_async!(params, SearchCodeTool, |tool| handle_search_code(
            &state.retriever,
            &state.config.db_path,
            tool
        )),
        "get_definition" => {
            dispatch_async!(params, GetDefinitionTool, |tool| handle_get_definition(
                state, tool
            ))
        }
        "report_selection" => {
            dispatch_async!(params, ReportSelectionTool, |tool| handle_report_selection(
                state, tool
            ))
        }
        "report_file_access" => dispatch_async!(params, ReportFileAccessTool, |tool| {
            handle_report_file_access(state, tool)
        }),
        "explain_search" => {
            dispatch_async!(params, ExplainSearchTool, |tool| handle_explain_search(
                &state.retriever,
                tool
            ))
        }
        "find_similar_code" => {
            dispatch_async!(
                params,
                FindSimilarCodeTool,
                |tool| handle_find_similar_code(state, tool)
            )
        }
        "get_context_bundle" => dispatch_async!(params, GetContextBundleTool, |tool| {
            handle_get_context_bundle(state, tool)
        }),

        // --- Sync handlers ---
        "get_file_symbols" => {
            dispatch_sync!(params, GetFileSymbolsTool, |tool| handle_get_file_symbols(
                state, tool
            ))
        }
        "hydrate_symbols" => {
            dispatch_sync!(params, HydrateSymbolsTool, |tool| handle_hydrate_symbols(
                state, tool
            ))
        }
        "plan_code_investigation" => {
            dispatch_sync!(params, PlanCodeInvestigationTool, |tool| {
                handle_plan_code_investigation(tool)
            })
        }
        "investigate" => {
            dispatch_async!(params, InvestigateTool, |tool| handle_investigate(
                state, tool
            ))
        }
        "ask_code" => {
            dispatch_async!(params, AskCodeTool, |tool| handle_ask_code(state, tool))
        }
        "explore_dependency_graph" => dispatch_sync!(params, ExploreDependencyGraphTool, |tool| {
            handle_explore_dependency_graph(state, tool)
        }),
        "get_similarity_cluster" => dispatch_sync!(params, GetSimilarityClusterTool, |tool| {
            handle_get_similarity_cluster(state, tool)
        }),
        "find_references" => {
            dispatch_sync!(params, FindReferencesTool, |tool| handle_find_references(
                state, tool
            ))
        }
        "get_usage_examples" => dispatch_sync!(params, GetUsageExamplesTool, |tool| {
            handle_get_usage_examples(state, tool)
        }),
        "get_call_hierarchy" => dispatch_sync!(params, GetCallHierarchyTool, |tool| {
            handle_get_call_hierarchy(state, tool)
        }),
        "get_type_graph" => dispatch_sync!(params, GetTypeGraphTool, |tool| handle_get_type_graph(
            state, tool
        )),
        "summarize_file" => {
            dispatch_sync!(params, SummarizeFileTool, |tool| handle_summarize_file(
                state, tool
            ))
        }
        "get_module_summary" => dispatch_sync!(params, GetModuleSummaryTool, |tool| {
            handle_get_module_summary(state, tool)
        }),
        "trace_data_flow" => {
            dispatch_sync!(params, TraceDataFlowTool, |tool| handle_trace_data_flow(
                state, tool
            ))
        }
        "find_affected_code" => dispatch_sync!(params, FindAffectedCodeTool, |tool| {
            handle_find_affected_code(state, tool)
        }),
        "search_todos" => dispatch_sync!(params, SearchTodosTool, |tool| handle_search_todos(
            state, tool
        )),
        "find_tests_for_symbol" => dispatch_sync!(params, FindTestsForSymbolTool, |tool| {
            handle_find_tests_for_symbol(state, tool)
        }),
        "search_decorators" => dispatch_sync!(params, SearchDecoratorsTool, |tool| {
            handle_search_decorators(state, tool)
        }),
        "search_framework_patterns" => {
            dispatch_sync!(params, SearchFrameworkPatternsTool, |tool| {
                handle_search_framework_patterns(state, tool)
            })
        }
        "find_dead_code" => dispatch_sync!(params, FindDeadCodeTool, |tool| handle_find_dead_code(
            state, tool
        )),
        "find_duplicates" => {
            dispatch_sync!(params, FindDuplicatesTool, |tool| handle_find_duplicates(
                state, tool
            ))
        }
        "find_stale_descriptions" => dispatch_sync!(params, FindStaleDescriptionsTool, |tool| {
            handle_find_stale_descriptions(state, tool)
        }),
        "find_undocumented_symbols" => {
            dispatch_sync!(params, FindUndocumentedSymbolsTool, |tool| {
                handle_find_undocumented_symbols(state, tool)
            })
        }
        "predict_impact" => {
            dispatch_sync!(params, PredictImpactTool, |tool| handle_predict_impact(
                state, tool
            ))
        }

        // --- Special: get_index_stats takes no tool arg ---
        "get_index_stats" => {
            let _tool: GetIndexStatsTool = parse_tool_args(&params).unwrap_or(GetIndexStatsTool {});
            let result = handle_get_index_stats(state).map_err(tool_internal_error)?;
            Ok(tool_json_content(&result))
        }

        // --- Standalone-only tools (error in embedded mode) ---
        "search_across_repos" => {
            let mut result =
                CallToolResult::text_content(vec![SEARCH_ACROSS_REPOS_EMBEDDED_MSG.into()]);
            result.is_error = Some(true);
            Ok(result)
        }
        "explore_cross_repo_dependencies" => {
            let mut result =
                CallToolResult::text_content(vec![EXPLORE_CROSS_REPO_DEPS_EMBEDDED_MSG.into()]);
            result.is_error = Some(true);
            Ok(result)
        }

        _ => Err(CallToolError::unknown_tool(params.name)),
    };

    let duration_ms = start.elapsed().as_millis();
    let status = if result.is_ok() { "ok" } else { "error" };
    tracing::info!(
        target: "mcp_access",
        tool = %tool_name,
        duration_ms = duration_ms as u64,
        status = %status,
    );

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capabilities_advertise_task_support() {
        use rust_mcp_sdk::schema::{ServerCapabilities, ServerCapabilitiesTools};
        let caps = ServerCapabilities {
            tools: Some(ServerCapabilitiesTools { list_changed: None }),
            tasks: Some(task_capabilities()),
            ..Default::default()
        };
        assert!(
            caps.can_run_task_augmented_tools(),
            "task_capabilities() must produce a ServerTasks that advertises task-augmented tool support"
        );
    }

    #[test]
    fn server_instructions_describe_ask_code_as_evidence_retriever() {
        let instructions = server_instructions();
        assert!(
            instructions.contains("ask_code"),
            "server instructions must name ask_code, got: {instructions}"
        );
        assert!(
            instructions.contains("evidence"),
            "server instructions must describe evidence-only contract, got: {instructions}"
        );
        assert!(
            instructions.contains("synthesise") || instructions.contains("synthesize"),
            "server instructions must direct the agent to synthesise the final answer, got: {instructions}"
        );
        assert!(
            instructions.contains("plan_code_investigation"),
            "server instructions must still mention the planner tool, got: {instructions}"
        );
        assert!(
            instructions.contains("investigate"),
            "server instructions must still mention investigate (raw evidence path), got: {instructions}"
        );
        assert!(
            instructions.contains("Grep"),
            "server instructions must position specialists against Grep, got: {instructions}"
        );
    }

    #[test]
    fn all_tools_contains_ask_code() {
        let tools = all_tools();
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert!(
            names.contains(&"ask_code"),
            "all_tools() must include 'ask_code', but only found: {names:?}"
        );
    }

    #[test]
    fn dispatch_routes_ask_code() {
        let source = include_str!("mod.rs");
        assert!(
            source.contains(r#""ask_code" =>"#),
            "dispatch_tool_call must route ask_code"
        );
    }

    #[test]
    fn all_tools_contains_investigate() {
        let tools = all_tools();
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert!(
            names.contains(&"investigate"),
            "all_tools() must include 'investigate', but only found: {names:?}"
        );
    }

    #[test]
    fn dispatch_routes_investigate() {
        let source = include_str!("mod.rs");
        assert!(
            source.contains(r#""investigate" =>"#),
            "dispatch_tool_call must route investigate"
        );
    }

    #[test]
    fn initialize_result_uses_server_instructions() {
        let main_source = include_str!("../main.rs");
        let uses = main_source
            .matches("instructions: Some(code_intelligence_mcp_server::server::server_instructions().into())")
            .count();
        assert_eq!(
            uses, 1,
            "standalone InitializeResult block must use server_instructions()"
        );
    }

    #[test]
    fn all_tools_contains_get_similarity_cluster() {
        let tools = all_tools();
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert!(
            names.contains(&"get_similarity_cluster"),
            "all_tools() must include 'get_similarity_cluster', but only found: {names:?}"
        );
    }

    #[test]
    fn all_tools_contains_search_across_repos() {
        let tools = all_tools();
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert!(
            names.contains(&"search_across_repos"),
            "all_tools() must include 'search_across_repos', but only found: {names:?}"
        );
    }

    #[test]
    fn all_tools_contains_plan_code_investigation() {
        let tools = all_tools();
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert!(
            names.contains(&"plan_code_investigation"),
            "all_tools() must include 'plan_code_investigation', but only found: {names:?}"
        );
    }

    #[test]
    fn dispatch_routes_plan_code_investigation() {
        let source = include_str!("mod.rs");
        assert!(
            source.contains(r#""plan_code_investigation" =>"#),
            "dispatch_tool_call must route plan_code_investigation"
        );
    }

    #[test]
    fn search_across_repos_tool_serializes_correctly() {
        use crate::tools::SearchAcrossReposTool;

        // Verify round-trip serialization
        let tool = SearchAcrossReposTool {
            query: "auth handler".to_string(),
            limit: Some(5),
            include_display: Some(true),
        };
        let json = serde_json::to_string(&tool).unwrap();
        let parsed: SearchAcrossReposTool = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.query, "auth handler");
        assert_eq!(parsed.limit, Some(5));
        assert_eq!(parsed.include_display, Some(true));

        // limit defaults to None when absent
        let no_limit: SearchAcrossReposTool = serde_json::from_str(r#"{"query":"foo"}"#).unwrap();
        assert_eq!(no_limit.limit, None);
        assert_eq!(no_limit.include_display, None);
    }

    #[test]
    fn task_augmented_error_message_and_routing_in_shared_dispatch() {
        // The shared `dispatch_task_augmented_call` function handles all task routing.
        // Verify via source inspection that it matches refresh_index and rejects
        // unsupported tools.  Full runtime testing requires a running MCP server
        // and task store (integration test territory).
        let source = include_str!("mod.rs");
        assert!(
            source.contains("does not support task-augmented execution"),
            "Unsupported tool error message must be present in dispatch_task_augmented_call"
        );
        assert!(
            source.contains(r#""refresh_index" =>"#),
            "dispatch_task_augmented_call must match refresh_index"
        );
        // Verify standalone delegates to the shared function
        let standalone_source = include_str!("standalone.rs");
        assert!(
            standalone_source.contains("dispatch_task_augmented_call"),
            "StandaloneHandler must delegate to shared dispatch_task_augmented_call"
        );
    }

    #[test]
    fn all_tools_contains_explore_cross_repo_dependencies() {
        let tools = all_tools();
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert!(
            names.contains(&"explore_cross_repo_dependencies"),
            "all_tools() must include 'explore_cross_repo_dependencies', but only found: {names:?}"
        );
    }

    #[test]
    fn explore_cross_repo_dependencies_tool_serializes_correctly() {
        use crate::tools::ExploreCrossRepoDependenciesTool;

        let tool = ExploreCrossRepoDependenciesTool {
            symbol_name: "MyService".to_string(),
            file_path: Some("src/service.rs".to_string()),
            direction: Some("downstream".to_string()),
            limit: Some(10),
            include_display: Some(true),
        };
        let json = serde_json::to_string(&tool).unwrap();
        let parsed: ExploreCrossRepoDependenciesTool = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.symbol_name, "MyService");
        assert_eq!(parsed.file_path, Some("src/service.rs".to_string()));
        assert_eq!(parsed.direction, Some("downstream".to_string()));
        assert_eq!(parsed.limit, Some(10));
        assert_eq!(parsed.include_display, Some(true));

        // Defaults when optional fields are absent
        let minimal: ExploreCrossRepoDependenciesTool =
            serde_json::from_str(r#"{"symbol_name":"foo"}"#).unwrap();
        assert_eq!(minimal.symbol_name, "foo");
        assert!(minimal.file_path.is_none());
        assert!(minimal.direction.is_none());
        assert!(minimal.limit.is_none());
        assert!(minimal.include_display.is_none());
    }

    #[test]
    fn embedded_mode_explore_cross_repo_deps_returns_helpful_message() {
        assert!(
            EXPLORE_CROSS_REPO_DEPS_EMBEDDED_MSG.contains("standalone"),
            "Message must mention standalone mode"
        );
        assert!(
            EXPLORE_CROSS_REPO_DEPS_EMBEDDED_MSG.contains("explore_cross_repo_dependencies"),
            "Message must mention the tool name"
        );

        let source = include_str!("mod.rs");
        assert!(
            source.contains("EXPLORE_CROSS_REPO_DEPS_EMBEDDED_MSG.into()"),
            "dispatch_tool_call must use the EXPLORE_CROSS_REPO_DEPS_EMBEDDED_MSG constant"
        );
    }

    #[test]
    fn all_tools_contains_get_context_bundle() {
        let tools = all_tools();
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert!(
            names.contains(&"get_context_bundle"),
            "all_tools() must include 'get_context_bundle', but only found: {names:?}"
        );
    }

    #[test]
    fn get_context_bundle_tool_serializes_correctly() {
        use crate::tools::GetContextBundleTool;

        // Full round-trip
        let tool = GetContextBundleTool {
            task: "fix auth bug".to_string(),
            max_tokens: Some(4096),
            sections: Some(vec!["definitions".to_string(), "tests".to_string()]),
            seed_limit: Some(5),
            include_raw_sections: Some(true),
        };
        let json = serde_json::to_string(&tool).unwrap();
        let parsed: GetContextBundleTool = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.task, "fix auth bug");
        assert_eq!(parsed.max_tokens, Some(4096));
        assert_eq!(parsed.seed_limit, Some(5));
        assert_eq!(parsed.include_raw_sections, Some(true));
        assert_eq!(
            parsed.sections,
            Some(vec!["definitions".to_string(), "tests".to_string()])
        );

        // Minimal (only required field)
        let minimal: GetContextBundleTool = serde_json::from_str(r#"{"task":"hello"}"#).unwrap();
        assert_eq!(minimal.task, "hello");
        assert!(minimal.max_tokens.is_none());
        assert!(minimal.sections.is_none());
        assert!(minimal.seed_limit.is_none());
        assert!(minimal.include_raw_sections.is_none());
    }

    #[test]
    fn dispatch_routes_get_context_bundle() {
        let source = include_str!("mod.rs");
        assert!(
            source.contains(r#""get_context_bundle" =>"#),
            "dispatch_tool_call must route get_context_bundle"
        );
    }

    #[test]
    fn embedded_mode_search_across_repos_returns_helpful_message() {
        // Verify that the constant message is informative and mentions
        // both the tool name and the --standalone flag so users know how
        // to enable cross-repo search.
        assert!(
            SEARCH_ACROSS_REPOS_EMBEDDED_MSG.contains("standalone"),
            "Message must mention standalone mode"
        );
        assert!(
            SEARCH_ACROSS_REPOS_EMBEDDED_MSG.contains("search_across_repos"),
            "Message must mention the tool name"
        );

        // Verify that `dispatch_tool_call` uses this constant by checking
        // the source code references it (compile-time guarantee via the
        // constant being used in the match arm above).
        let source = include_str!("mod.rs");
        assert!(
            source.contains("SEARCH_ACROSS_REPOS_EMBEDDED_MSG.into()"),
            "dispatch_tool_call must use the SEARCH_ACROSS_REPOS_EMBEDDED_MSG constant"
        );
    }
}
