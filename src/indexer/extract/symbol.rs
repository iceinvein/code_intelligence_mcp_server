use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SymbolKind {
    Function,
    Class,
    Interface,
    TypeAlias,
    Enum,
    Const,
    Struct,
    Trait,
    Impl,
    Module,
}

/// Data flow edge types for tracking reads/writes relationships
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DataFlowType {
    /// x = foo() -> foo is read
    Reads,
    /// x = 1 -> x is written
    Writes,
}

/// Data flow edge representing reads/writes relationships between symbols
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DataFlowEdge {
    /// The symbol being read/written (e.g., variable name on right or left side)
    pub from_symbol: String,
    /// The context symbol (function/method where this occurs)
    pub to_symbol: String,
    /// Type of data flow
    pub flow_type: DataFlowType,
    /// Line number where this flow occurs
    pub at_line: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ByteSpan {
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineSpan {
    pub start: u32,
    pub end: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtractedSymbol {
    pub name: String,
    pub kind: SymbolKind,
    pub exported: bool,
    pub bytes: ByteSpan,
    pub lines: LineSpan,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Import {
    pub name: String,
    pub source: String,
    pub alias: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtractedFile {
    pub symbols: Vec<ExtractedSymbol>,
    pub imports: Vec<Import>,
    pub type_edges: Vec<(String, String)>, // (parent_symbol_name, type_name)
    pub dataflow_edges: Vec<DataFlowEdge>, // Data flow edges (reads/writes)
}
