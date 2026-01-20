//! CLI argument parsing and help text

pub fn wants_help(args: &[String]) -> bool {
    args.iter()
        .skip(1)
        .any(|a| a == "-h" || a == "--help" || a == "help")
}

pub fn wants_version(args: &[String]) -> bool {
    args.iter()
        .skip(1)
        .any(|a| a == "-V" || a == "--version" || a == "version")
}

pub fn print_help() {
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

pub fn print_version() {
    println!("{}", env!("CARGO_PKG_VERSION"));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wants_help_and_version_detect_common_flags() {
        assert!(wants_help(&["bin".to_string(), "--help".to_string()]));
        assert!(wants_help(&["bin".to_string(), "-h".to_string()]));
        assert!(wants_version(&["bin".to_string(), "--version".to_string()]));
        assert!(wants_version(&["bin".to_string(), "-V".to_string()]));
        assert!(!wants_help(&["bin".to_string()]));
        assert!(!wants_version(&["bin".to_string()]));
    }
}
