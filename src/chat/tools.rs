//! Tool definitions and dispatch for the chat agent loop.
//!
//! Defines which MCP tools the chat LLM can call, their JSON schemas
//! for the Qwen2.5 system prompt, and dispatches parsed tool calls
//! to the existing handler functions in `src/handlers/mod.rs`.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::handlers::AppState;

/// A parsed tool call from LLM output.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub name: String,
    pub arguments: Value,
}

/// Generate the JSON tool definitions for the Qwen2.5 system prompt.
///
/// Returns a `Vec` of tool definition JSON objects to embed in `<tools>...</tools>` tags.
///
/// # Examples
///
/// ```
/// let defs = code_intelligence_mcp_server::chat::tools::tool_definitions();
/// assert_eq!(defs.len(), 11);
/// assert_eq!(defs[0]["function"]["name"], "search_code");
/// ```
pub fn tool_definitions() -> Vec<Value> {
    vec![
        json!({
            "type": "function",
            "function": {
                "name": "search_code",
                "description": "Search the codebase for symbols and return assembled context. \
                    Use this for broad queries like 'how does auth work?' or 'find the database layer'.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "Natural language or keyword search query"
                        },
                        "limit": {
                            "type": "integer",
                            "description": "Maximum number of results to return (default: 5)"
                        },
                        "exported_only": {
                            "type": "boolean",
                            "description": "If true, restrict results to exported/public symbols only"
                        }
                    },
                    "required": ["query"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "get_definition",
                "description": "Get the full definition(s) of a symbol by exact name. \
                    Use this when you already know the symbol name and need its source code.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "symbol_name": {
                            "type": "string",
                            "description": "The exact symbol name to look up (e.g. 'handle_search_code', 'AppState')"
                        },
                        "file": {
                            "type": "string",
                            "description": "Optional file path to disambiguate when multiple symbols share the same name"
                        },
                        "limit": {
                            "type": "integer",
                            "description": "Maximum number of definitions to return (default: 10)"
                        }
                    },
                    "required": ["symbol_name"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "find_references",
                "description": "Find all imports, usages, and call sites of a symbol across the codebase.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "symbol_name": {
                            "type": "string",
                            "description": "The symbol name to find references for"
                        },
                        "file": {
                            "type": "string",
                            "description": "Optional file path to disambiguate when multiple symbols share the same name"
                        },
                        "reference_type": {
                            "type": "string",
                            "description": "Filter by reference type: 'call', 'import', 'reference', 'extends', 'implements', or 'all' (default)",
                            "enum": ["call", "import", "reference", "extends", "implements", "all"]
                        },
                        "limit": {
                            "type": "integer",
                            "description": "Maximum number of references to return (default: 200)"
                        }
                    },
                    "required": ["symbol_name"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "get_call_hierarchy",
                "description": "Return the call hierarchy rooted at a symbol — who calls it (callers) \
                    or what it calls (callees), or both.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "symbol_name": {
                            "type": "string",
                            "description": "The symbol name to build the call hierarchy for"
                        },
                        "direction": {
                            "type": "string",
                            "description": "Traversal direction: 'callers' (who calls this), 'callees' (what this calls), or 'both' (default)",
                            "enum": ["callers", "callees", "both"]
                        },
                        "depth": {
                            "type": "integer",
                            "description": "Maximum traversal depth (default: 3)"
                        },
                        "limit": {
                            "type": "integer",
                            "description": "Maximum number of nodes to return (default: 50)"
                        }
                    },
                    "required": ["symbol_name"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "get_type_graph",
                "description": "Return type relationships for a symbol: inheritance, interface implementation, \
                    and type aliases — upstream (who extends this) or downstream (what this extends).",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "symbol_name": {
                            "type": "string",
                            "description": "The type/struct/class/interface name to inspect"
                        },
                        "direction": {
                            "type": "string",
                            "description": "Traversal direction: 'downstream' (what does this extend/implement), 'upstream' (who extends/implements this), or 'both' (default)",
                            "enum": ["downstream", "upstream", "both"]
                        },
                        "depth": {
                            "type": "integer",
                            "description": "Maximum traversal depth (default: 3)"
                        },
                        "limit": {
                            "type": "integer",
                            "description": "Maximum number of nodes to return (default: 50)"
                        }
                    },
                    "required": ["symbol_name"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "explore_dependency_graph",
                "description": "Explore module-level import/export dependencies upstream or downstream \
                    from a symbol to understand the dependency graph.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "symbol_name": {
                            "type": "string",
                            "description": "The symbol or module name to explore dependencies for"
                        },
                        "direction": {
                            "type": "string",
                            "description": "Traversal direction: 'upstream' (dependencies this relies on), 'downstream' (what depends on this), or 'both' (default)",
                            "enum": ["upstream", "downstream", "both"]
                        },
                        "depth": {
                            "type": "integer",
                            "description": "Maximum traversal depth (default: 3)"
                        },
                        "limit": {
                            "type": "integer",
                            "description": "Maximum number of nodes to return (default: 50)"
                        }
                    },
                    "required": ["symbol_name"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "get_file_symbols",
                "description": "List all symbols defined in a specific file without returning full definitions. \
                    Useful for getting an overview of a file's contents.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "file_path": {
                            "type": "string",
                            "description": "Path to the file (relative to repo root, e.g. 'src/handlers/mod.rs')"
                        },
                        "exported_only": {
                            "type": "boolean",
                            "description": "If true, return only exported/public symbols"
                        }
                    },
                    "required": ["file_path"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "find_affected_code",
                "description": "Find code that would be affected if the given symbol changes — \
                    its reverse dependencies (who calls or imports it).",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "symbol_name": {
                            "type": "string",
                            "description": "The symbol name to perform impact analysis on"
                        },
                        "file_path": {
                            "type": "string",
                            "description": "Optional file path to disambiguate when multiple symbols share the same name"
                        },
                        "depth": {
                            "type": "integer",
                            "description": "Maximum traversal depth (default: 3)"
                        },
                        "limit": {
                            "type": "integer",
                            "description": "Maximum number of affected symbols to return (default: 50)"
                        },
                        "include_tests": {
                            "type": "boolean",
                            "description": "If true, include test files in the results (default: false)"
                        },
                        "edge_types": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "Filter by edge types (default: [\"call\", \"reference\"]). Options: call, reference, type, extends, implements, alias"
                        }
                    },
                    "required": ["symbol_name"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "trace_data_flow",
                "description": "Trace variable reads and writes through the codebase to understand \
                    data flow — where a variable is written and where it is read.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "symbol_name": {
                            "type": "string",
                            "description": "The variable or symbol name to trace"
                        },
                        "file_path": {
                            "type": "string",
                            "description": "Optional file path to scope the trace"
                        },
                        "direction": {
                            "type": "string",
                            "description": "Trace direction: 'reads' (find where value is read), 'writes' (find where value is written), or 'both' (default)",
                            "enum": ["reads", "writes", "both"]
                        },
                        "depth": {
                            "type": "integer",
                            "description": "Maximum traversal depth (default: 3)"
                        },
                        "limit": {
                            "type": "integer",
                            "description": "Maximum number of results to return (default: 50)"
                        }
                    },
                    "required": ["symbol_name"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "summarize_file",
                "description": "Generate a structural summary of a file: symbol counts, key exports, \
                    and optionally full signatures.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "file_path": {
                            "type": "string",
                            "description": "Path to the file (relative to repo root, e.g. 'src/retrieval/mod.rs')"
                        },
                        "include_signatures": {
                            "type": "boolean",
                            "description": "If true, include function/method signatures in the summary"
                        },
                        "verbose": {
                            "type": "boolean",
                            "description": "If true, include full symbol text in the summary"
                        }
                    },
                    "required": ["file_path"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "get_context_bundle",
                "description": "Get a pre-assembled context bundle for a task. Returns definitions, \
                    call chains, tests, similar code, and affected code in one call.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "task": {
                            "type": "string",
                            "description": "Description of the task to gather context for"
                        },
                        "sections": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "Which sections to include: definitions, call_chain, tests, similar, affected. Default: all."
                        },
                        "max_tokens": {
                            "type": "integer",
                            "description": "Maximum tokens for context output (default: unlimited)"
                        }
                    },
                    "required": ["task"]
                }
            }
        }),
    ]
}

/// Execute a parsed tool call against the `AppState`.
///
/// Dispatches to the appropriate handler in `src/handlers/mod.rs` and returns
/// the result as a JSON string. Results exceeding 4 000 characters are truncated
/// so they fit within the LLM's 8 192-token context window.
///
/// # Errors
///
/// Returns an error if the underlying handler returns one. Unknown tool names
/// do not error — they return a JSON error object instead so the LLM can
/// recover gracefully.
pub async fn execute_tool(state: &AppState, tool_call: &ToolCall) -> Result<String> {
    use crate::handlers::*;
    use crate::tools::*;

    let args = &tool_call.arguments;

    let result: Value = match tool_call.name.as_str() {
        "search_code" => {
            let tool = SearchCodeTool {
                query: args["query"].as_str().unwrap_or("").to_string(),
                limit: args.get("limit").and_then(|v| v.as_u64()).map(|v| v as u32),
                exported_only: args.get("exported_only").and_then(|v| v.as_bool()),
            };
            handle_search_code(&state.retriever, tool).await?
        }
        "get_definition" => {
            let tool = GetDefinitionTool {
                symbol_name: args["symbol_name"].as_str().unwrap_or("").to_string(),
                file: args.get("file").and_then(|v| v.as_str()).map(str::to_string),
                limit: args.get("limit").and_then(|v| v.as_u64()).map(|v| v as u32),
            };
            handle_get_definition(state, tool).await?
        }
        "find_references" => {
            let tool = FindReferencesTool {
                symbol_name: args["symbol_name"].as_str().unwrap_or("").to_string(),
                file: args.get("file").and_then(|v| v.as_str()).map(str::to_string),
                reference_type: args
                    .get("reference_type")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
                limit: args.get("limit").and_then(|v| v.as_u64()).map(|v| v as u32),
            };
            handle_find_references(state, tool)?
        }
        "get_call_hierarchy" => {
            let tool = GetCallHierarchyTool {
                symbol_name: args["symbol_name"].as_str().unwrap_or("").to_string(),
                direction: args
                    .get("direction")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
                depth: args.get("depth").and_then(|v| v.as_u64()).map(|v| v as u32),
                limit: args.get("limit").and_then(|v| v.as_u64()).map(|v| v as u32),
            };
            handle_get_call_hierarchy(state, tool)?
        }
        "get_type_graph" => {
            let tool = GetTypeGraphTool {
                symbol_name: args["symbol_name"].as_str().unwrap_or("").to_string(),
                direction: args
                    .get("direction")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
                depth: args.get("depth").and_then(|v| v.as_u64()).map(|v| v as u32),
                limit: args.get("limit").and_then(|v| v.as_u64()).map(|v| v as u32),
            };
            handle_get_type_graph(state, tool)?
        }
        "explore_dependency_graph" => {
            let tool = ExploreDependencyGraphTool {
                symbol_name: args["symbol_name"].as_str().unwrap_or("").to_string(),
                direction: args
                    .get("direction")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
                depth: args.get("depth").and_then(|v| v.as_u64()).map(|v| v as u32),
                limit: args.get("limit").and_then(|v| v.as_u64()).map(|v| v as u32),
            };
            handle_explore_dependency_graph(state, tool)?
        }
        "get_file_symbols" => {
            let tool = GetFileSymbolsTool {
                file_path: args["file_path"].as_str().unwrap_or("").to_string(),
                exported_only: args.get("exported_only").and_then(|v| v.as_bool()),
            };
            handle_get_file_symbols(state, tool)?
        }
        "find_affected_code" => {
            let tool = FindAffectedCodeTool {
                symbol_name: args["symbol_name"].as_str().unwrap_or("").to_string(),
                file_path: args
                    .get("file_path")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
                depth: args.get("depth").and_then(|v| v.as_u64()).map(|v| v as u32),
                limit: args.get("limit").and_then(|v| v.as_u64()).map(|v| v as u32),
                include_tests: args.get("include_tests").and_then(|v| v.as_bool()),
                edge_types: args
                    .get("edge_types")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(str::to_string))
                            .collect()
                    }),
            };
            handle_find_affected_code(state, tool)?
        }
        "trace_data_flow" => {
            let tool = TraceDataFlowTool {
                symbol_name: args["symbol_name"].as_str().unwrap_or("").to_string(),
                file_path: args
                    .get("file_path")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
                direction: args
                    .get("direction")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
                depth: args.get("depth").and_then(|v| v.as_u64()).map(|v| v as u32),
                limit: args.get("limit").and_then(|v| v.as_u64()).map(|v| v as u32),
                inter_procedural: args.get("inter_procedural").and_then(|v| v.as_bool()),
            };
            handle_trace_data_flow(state, tool)?
        }
        "summarize_file" => {
            let tool = SummarizeFileTool {
                file_path: args["file_path"].as_str().unwrap_or("").to_string(),
                include_signatures: args.get("include_signatures").and_then(|v| v.as_bool()),
                verbose: args.get("verbose").and_then(|v| v.as_bool()),
            };
            handle_summarize_file(state, tool)?
        }
        "get_context_bundle" => {
            let tool = GetContextBundleTool {
                task: args.get("task").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                max_tokens: args.get("max_tokens").and_then(|v| v.as_u64()).map(|v| v as u32),
                sections: args.get("sections").and_then(|v| v.as_array()).map(|arr| {
                    arr.iter()
                        .filter_map(|s| s.as_str().map(String::from))
                        .collect()
                }),
                seed_limit: args.get("seed_limit").and_then(|v| v.as_u64()).map(|v| v as u32),
            };
            let result = handle_get_context_bundle(state, tool).await?;
            // Return the context field directly for chat (more compact)
            let result_str = serde_json::to_string_pretty(&result)?;
            if result_str.len() > 4000 {
                return Ok(format!(
                    "{}... [truncated, {} bytes total]",
                    &result_str[..4000],
                    result_str.len()
                ));
            }
            return Ok(result_str);
        }
        unknown => json!({"error": format!("Unknown tool: {}", unknown)}),
    };

    // Truncate large results to fit in LLM context window.
    // With an 8 192-token budget shared across the full conversation, 4 000 characters
    // is a conservative upper bound for a single tool result.
    let result_str = serde_json::to_string(&result)?;
    if result_str.len() > 4000 {
        Ok(format!(
            "{}... [truncated, {} bytes total]",
            &result_str[..4000],
            result_str.len()
        ))
    } else {
        Ok(result_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_definitions_returns_eleven_tools() {
        let defs = tool_definitions();
        assert_eq!(defs.len(), 11);
    }

    #[test]
    fn tool_definitions_have_required_structure() {
        let defs = tool_definitions();
        for def in &defs {
            assert_eq!(def["type"], "function", "tool must have type=function");
            assert!(
                def["function"]["name"].is_string(),
                "tool must have a name string"
            );
            assert!(
                def["function"]["description"].is_string(),
                "tool must have a description string"
            );
            assert!(
                def["function"]["parameters"].is_object(),
                "tool must have parameters object"
            );
        }
    }

    #[test]
    fn tool_definitions_cover_expected_names() {
        let defs = tool_definitions();
        let names: Vec<&str> = defs
            .iter()
            .map(|d| d["function"]["name"].as_str().unwrap())
            .collect();

        assert!(names.contains(&"search_code"));
        assert!(names.contains(&"get_definition"));
        assert!(names.contains(&"find_references"));
        assert!(names.contains(&"get_call_hierarchy"));
        assert!(names.contains(&"get_type_graph"));
        assert!(names.contains(&"explore_dependency_graph"));
        assert!(names.contains(&"get_file_symbols"));
        assert!(names.contains(&"find_affected_code"));
        assert!(names.contains(&"trace_data_flow"));
        assert!(names.contains(&"summarize_file"));
        assert!(names.contains(&"get_context_bundle"));
    }

    #[test]
    fn all_tools_have_required_fields_defined() {
        let defs = tool_definitions();
        for def in &defs {
            let params = &def["function"]["parameters"];
            assert_eq!(
                params["type"], "object",
                "parameters must be of type object for tool {}",
                def["function"]["name"]
            );
            assert!(
                params["required"].is_array(),
                "parameters must have a required array for tool {}",
                def["function"]["name"]
            );
        }
    }
}
