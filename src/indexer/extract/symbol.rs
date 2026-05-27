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
    /// Object-literal property identifier carrying a hook-shaped name
    /// (onX/beforeX/afterX/willX/didX/handleX), a function-valued slot,
    /// or a literal in an exported const-object. Lets queries like
    /// "where is onBeforeToolUse defined" land on the precise file:line
    /// of the property, not just the enclosing object's const symbol.
    Property,
}

// TodoKind, TodoEntry, JSDocEntry, JSDocParam moved to crate::storage::sqlite::schema
// Re-exported here for backward compatibility with extractors.
pub use crate::storage::sqlite::schema::{JSDocEntry, JSDocParam, TodoEntry, TodoKind};

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

/// Framework pattern kind for Elysia/Hono/Express/Fastify/NestJS/tRPC/Next.js/Convex
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FrameworkPatternKind {
    Route,
    WebSocket,
    Macro,
    Plugin,
    State,
    Decorate,
    Derive,
    Resolve,
    Guard,
    Group,
    Listen,
    Middleware,
    Controller,
    Injectable,
    Module,
    Pipe,
    Interceptor,
    Procedure,
    Router,
    ErrorHandler,
    Hook,
    Schema,
    FileRoute,
    Query,
    Mutation,
    Action,
    CronJob,
}

impl std::fmt::Display for FrameworkPatternKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Route => write!(f, "route"),
            Self::WebSocket => write!(f, "websocket"),
            Self::Macro => write!(f, "macro"),
            Self::Plugin => write!(f, "plugin"),
            Self::State => write!(f, "state"),
            Self::Decorate => write!(f, "decorate"),
            Self::Derive => write!(f, "derive"),
            Self::Resolve => write!(f, "resolve"),
            Self::Guard => write!(f, "guard"),
            Self::Group => write!(f, "group"),
            Self::Listen => write!(f, "listen"),
            Self::Middleware => write!(f, "middleware"),
            Self::Controller => write!(f, "controller"),
            Self::Injectable => write!(f, "injectable"),
            Self::Module => write!(f, "module"),
            Self::Pipe => write!(f, "pipe"),
            Self::Interceptor => write!(f, "interceptor"),
            Self::Procedure => write!(f, "procedure"),
            Self::Router => write!(f, "router"),
            Self::ErrorHandler => write!(f, "error_handler"),
            Self::Hook => write!(f, "hook"),
            Self::Schema => write!(f, "schema"),
            Self::FileRoute => write!(f, "file_route"),
            Self::Query => write!(f, "query"),
            Self::Mutation => write!(f, "mutation"),
            Self::Action => write!(f, "action"),
            Self::CronJob => write!(f, "cron_job"),
        }
    }
}

/// Extracted framework pattern from source code
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtractedFrameworkPattern {
    pub line: u32,
    pub column: u32, // Column for ordering chained methods on same line
    pub framework: String,
    pub kind: FrameworkPatternKind,
    pub http_method: Option<String>,
    pub path: Option<String>,
    pub name: Option<String>,
    pub handler: Option<String>,
    pub arguments: Option<String>,
    pub parent_chain: Option<String>,
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
    /// Enclosing function/method name for local variable resolution
    pub scope: Option<String>,
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
    /// Framework patterns extracted from this file (Elysia/Hono/Express)
    pub framework_patterns: Vec<ExtractedFrameworkPattern>,
}
