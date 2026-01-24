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

/// TODO/FIXME comment kind for technical debt tracking (LANG-03)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TodoKind {
    Todo,
    Fixme,
}

/// TODO/FIXME comment entry extracted from source code (LANG-03)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TodoEntry {
    pub kind: TodoKind,
    pub text: String,
    pub file_path: String,
    pub line: u32,
    pub associated_symbol: Option<String>, // Symbol immediately following the TODO
}

/// JSDoc comment entry for TypeScript/JavaScript documentation (LANG-01)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JSDocEntry {
    pub symbol_id: String,
    pub raw_text: String,
    pub summary: Option<String>,
    pub params: Vec<JSDocParam>,
    pub returns: Option<String>,
    pub examples: Vec<String>,
    pub deprecated: bool,
    pub throws: Vec<String>,
    pub see_also: Vec<String>,
    pub since: Option<String>,
}

/// JSDoc parameter documentation (LANG-01)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JSDocParam {
    pub name: String,
    pub type_annotation: Option<String>,
    pub description: Option<String>,
}

/// TypeScript/JavaScript decorator entry for framework metadata (LANG-02)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecoratorEntry {
    pub symbol_id: String,
    pub name: String,
    pub arguments: Option<String>,
    pub target_line: u32,
    pub decorator_type: DecoratorType,
}

/// Type of decorator for framework-specific identification (LANG-02)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecoratorType {
    // Angular
    Component,
    Injectable,
    Module,
    Directive,
    Pipe,
    // NestJS
    Controller,
    Get,
    Post,
    Put,
    Delete,
    Patch,
    Param,
    Body,
    Query,
    // Generic/Unknown
    ClassDecorator,
    MethodDecorator,
    PropertyDecorator,
    ParameterDecorator,
    Unknown,
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
    /// TODO/FIXME comments extracted from this file (LANG-03)
    pub todos: Vec<TodoEntry>,
    /// JSDoc comments extracted from this file (LANG-01)
    pub jsdoc_entries: Vec<JSDocEntry>,
    /// Decorators extracted from this file (LANG-02)
    pub decorators: Vec<DecoratorEntry>,
}
