//! Next.js App Router framework pattern extraction
//!
//! Next.js App Router uses file-path conventions — the route structure is
//! encoded in directory names rather than explicit route registration calls.
//! This extractor combines file-path analysis with AST-level export detection.
//!
//! # File conventions
//!
//! | File name pattern         | What it produces                           |
//! |---------------------------|---------------------------------------------|
//! | `app/**/route.ts[x]`     | HTTP handler per exported `GET`/`POST`/… fn |
//! | `app/**/page.tsx`        | `FileRoute` (name="page")                   |
//! | `app/**/layout.tsx`      | `FileRoute` (name="layout")                 |
//! | `app/**/error.tsx`       | `ErrorHandler` (name="error")               |
//! | `app/**/loading.tsx`     | `FileRoute` (name="loading")                |
//! | `app/**/not-found.tsx`   | `ErrorHandler` (name="not-found")           |
//! | `middleware.ts[x]`       | `Middleware` for exported `middleware` fn   |
//!
//! Dynamic segments like `[id]`, `[...slug]`, and `[[...catch]]` are
//! converted to `:id`, `*slug`, and `*catch` respectively by
//! [`derive_nextjs_route_path`].
//!
//! Route groups `(group-name)` are transparent — they do not appear in the
//! derived URL path.

use tree_sitter::Node;

use super::framework_utils::{derive_nextjs_route_path, find_named_exports, text_for_node};
use super::symbol::{ExtractedFrameworkPattern, FrameworkPatternKind};

/// HTTP method names that are valid route handler exports in `route.ts` files.
const ROUTE_HTTP_METHODS: &[&str] = &[
    "GET", "POST", "PUT", "DELETE", "PATCH", "OPTIONS", "HEAD",
];

/// Return `true` when the file at `file_path` follows a Next.js App Router
/// convention and should be processed by this extractor.
///
/// # Examples
/// ```
/// use code_intelligence_mcp_server::indexer::extract::nextjs::is_nextjs_convention_file;
/// assert!(is_nextjs_convention_file("app/api/users/route.ts"));
/// assert!(is_nextjs_convention_file("src/app/dashboard/page.tsx"));
/// assert!(is_nextjs_convention_file("middleware.ts"));
/// assert!(!is_nextjs_convention_file("src/utils/helpers.ts"));
/// ```
pub fn is_nextjs_convention_file(file_path: &str) -> bool {
    let path = file_path.replace('\\', "/");

    // Standalone `middleware.ts` / `middleware.tsx` at any depth.
    if is_middleware_file(&path) {
        return true;
    }

    // Must live somewhere inside an `app/` directory.
    if !is_in_app_dir(&path) {
        return false;
    }

    // Check the terminal file name.
    let file_name = path.rsplit('/').next().unwrap_or("");
    matches!(
        file_name,
        "route.ts"
            | "route.tsx"
            | "page.tsx"
            | "page.jsx"
            | "layout.tsx"
            | "layout.jsx"
            | "error.tsx"
            | "error.jsx"
            | "loading.tsx"
            | "loading.jsx"
            | "not-found.tsx"
            | "not-found.jsx"
            | "template.tsx"
            | "template.jsx"
    )
}

/// Extract Next.js App Router patterns from an AST.
///
/// `file_path` must be provided so that URL paths can be derived from the
/// directory structure.  Use [`is_nextjs_convention_file`] to pre-filter files
/// before calling this function.
pub fn extract_nextjs_patterns(
    root: Node,
    source: &str,
    file_path: &str,
) -> Vec<ExtractedFrameworkPattern> {
    let path_norm = file_path.replace('\\', "/");
    let mut patterns = Vec::new();

    if is_middleware_file(&path_norm) {
        extract_middleware_patterns(root, source, &mut patterns);
        return patterns;
    }

    if !is_in_app_dir(&path_norm) {
        return patterns;
    }

    let file_name = path_norm.rsplit('/').next().unwrap_or("");
    let base_name = file_name
        .strip_suffix(".tsx")
        .or_else(|| file_name.strip_suffix(".jsx"))
        .or_else(|| file_name.strip_suffix(".ts"))
        .unwrap_or(file_name);

    match base_name {
        "route" => extract_route_handlers(root, source, &path_norm, &mut patterns),
        "page" => extract_file_convention(
            root,
            source,
            &path_norm,
            FrameworkPatternKind::FileRoute,
            "page",
            &mut patterns,
        ),
        "layout" => extract_file_convention(
            root,
            source,
            &path_norm,
            FrameworkPatternKind::FileRoute,
            "layout",
            &mut patterns,
        ),
        "loading" | "template" => extract_file_convention(
            root,
            source,
            &path_norm,
            FrameworkPatternKind::FileRoute,
            base_name,
            &mut patterns,
        ),
        "error" | "not-found" => extract_file_convention(
            root,
            source,
            &path_norm,
            FrameworkPatternKind::ErrorHandler,
            base_name,
            &mut patterns,
        ),
        _ => {}
    }

    patterns.sort_by_key(|p| (p.line, p.column));
    patterns
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// `true` when `path` is (or ends with) `middleware.ts` / `middleware.tsx`.
fn is_middleware_file(path: &str) -> bool {
    let file_name = path.rsplit('/').next().unwrap_or(path);
    file_name == "middleware.ts" || file_name == "middleware.tsx"
}

/// `true` when `path` sits under an `app/` directory segment.
fn is_in_app_dir(path: &str) -> bool {
    path.starts_with("app/") || path.contains("/app/")
}

/// Scan named exports in a `route.ts` file and emit a `Route` pattern for each
/// exported HTTP-method handler (`GET`, `POST`, etc.).
fn extract_route_handlers(
    root: Node,
    source: &str,
    file_path: &str,
    patterns: &mut Vec<ExtractedFrameworkPattern>,
) {
    let url_path = derive_nextjs_route_path(file_path);
    let exports = find_named_exports(root, source);

    for (export_name, export_node) in exports {
        let upper = export_name.to_uppercase();
        if !ROUTE_HTTP_METHODS.contains(&upper.as_str()) {
            continue;
        }

        let pos = export_node.start_position();
        patterns.push(ExtractedFrameworkPattern {
            line: pos.row as u32 + 1,
            column: pos.column as u32,
            framework: "nextjs".to_string(),
            kind: FrameworkPatternKind::Route,
            http_method: Some(upper),
            path: url_path.clone(),
            name: None,
            handler: Some(export_name),
            arguments: None,
            parent_chain: None,
        });
    }
}

/// Emit a single `FileRoute` or `ErrorHandler` pattern for convention files
/// that have a default export (`page.tsx`, `layout.tsx`, `error.tsx`, …).
fn extract_file_convention(
    root: Node,
    source: &str,
    file_path: &str,
    kind: FrameworkPatternKind,
    convention_name: &str,
    patterns: &mut Vec<ExtractedFrameworkPattern>,
) {
    let url_path = derive_nextjs_route_path(file_path);
    let exports = find_named_exports(root, source);

    // Look for a default export first; if not found, accept any export from this
    // file (some pages use named exports with file-level conventions).
    let default_export = exports
        .iter()
        .find(|(name, _)| name == "default")
        .or_else(|| exports.first());

    if let Some((_, export_node)) = default_export {
        let pos = export_node.start_position();
        patterns.push(ExtractedFrameworkPattern {
            line: pos.row as u32 + 1,
            column: pos.column as u32,
            framework: "nextjs".to_string(),
            kind,
            http_method: None,
            path: url_path,
            name: Some(convention_name.to_string()),
            handler: None,
            arguments: None,
            parent_chain: None,
        });
    }
}

/// Scan a `middleware.ts` file for the exported `middleware` function.
fn extract_middleware_patterns(
    root: Node,
    source: &str,
    patterns: &mut Vec<ExtractedFrameworkPattern>,
) {
    let exports = find_named_exports(root, source);

    for (export_name, export_node) in &exports {
        if export_name == "middleware" || export_name == "default" {
            let pos = export_node.start_position();
            patterns.push(ExtractedFrameworkPattern {
                line: pos.row as u32 + 1,
                column: pos.column as u32,
                framework: "nextjs".to_string(),
                kind: FrameworkPatternKind::Middleware,
                http_method: None,
                path: None,
                name: Some("middleware".to_string()),
                handler: Some(export_name.clone()),
                arguments: None,
                parent_chain: None,
            });
            // Only emit one middleware pattern per file.
            break;
        }
    }

    // Fallback: scan the raw AST for a top-level `function middleware` even when
    // it is not inside an `export_statement` (re-exported via `config` pattern).
    if patterns.is_empty() {
        scan_for_middleware_function(root, source, patterns);
    }
}

/// Walk immediate children of `root` to find a plain `function middleware`
/// declaration that may not be wrapped in an `export_statement`.
fn scan_for_middleware_function(
    root: Node,
    source: &str,
    patterns: &mut Vec<ExtractedFrameworkPattern>,
) {
    let mut cursor = root.walk();
    for child in root.children(&mut cursor) {
        if child.kind() == "function_declaration" {
            if let Some(name_node) = child.child_by_field_name("name") {
                let name = text_for_node(name_node, source);
                if name == "middleware" {
                    let pos = child.start_position();
                    patterns.push(ExtractedFrameworkPattern {
                        line: pos.row as u32 + 1,
                        column: pos.column as u32,
                        framework: "nextjs".to_string(),
                        kind: FrameworkPatternKind::Middleware,
                        http_method: None,
                        path: None,
                        name: Some("middleware".to_string()),
                        handler: Some("middleware".to_string()),
                        arguments: None,
                        parent_chain: None,
                    });
                    break;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::indexer::parser::{parser_for_id, LanguageId};

    fn parse_ts(source: &str) -> tree_sitter::Tree {
        let mut parser = parser_for_id(LanguageId::Typescript).unwrap();
        parser.parse(source, None).unwrap()
    }

    fn parse_tsx(source: &str) -> tree_sitter::Tree {
        let mut parser = parser_for_id(LanguageId::Tsx).unwrap();
        parser.parse(source, None).unwrap()
    }

    #[test]
    fn detects_convention_files() {
        assert!(is_nextjs_convention_file("app/api/users/route.ts"));
        assert!(is_nextjs_convention_file("app/dashboard/page.tsx"));
        assert!(is_nextjs_convention_file("app/layout.tsx"));
        assert!(is_nextjs_convention_file("app/error.tsx"));
        assert!(is_nextjs_convention_file("app/loading.tsx"));
        assert!(is_nextjs_convention_file("src/app/api/auth/route.ts"));
        assert!(is_nextjs_convention_file("middleware.ts"));
        assert!(is_nextjs_convention_file("src/middleware.ts"));

        // Non-convention files must return false.
        assert!(!is_nextjs_convention_file("src/utils/helpers.ts"));
        assert!(!is_nextjs_convention_file("components/Button.tsx"));
        assert!(!is_nextjs_convention_file("pages/index.tsx")); // Pages Router, not App Router
        assert!(!is_nextjs_convention_file("app/utils/format.ts"));
    }

    #[test]
    fn extracts_route_handlers() {
        let source = r#"
import { NextRequest, NextResponse } from 'next/server'

export async function GET(request: NextRequest) {
  return NextResponse.json({ users: [] })
}

export async function POST(request: NextRequest) {
  const body = await request.json()
  return NextResponse.json({ id: '1' }, { status: 201 })
}
"#;
        let tree = parse_ts(source);
        let patterns =
            extract_nextjs_patterns(tree.root_node(), source, "app/api/users/route.ts");

        let route_patterns: Vec<_> = patterns
            .iter()
            .filter(|p| p.kind == FrameworkPatternKind::Route)
            .collect();
        assert_eq!(route_patterns.len(), 2, "Expected 2 route handler patterns");

        let get_route = route_patterns
            .iter()
            .find(|p| p.http_method == Some("GET".to_string()));
        assert!(get_route.is_some(), "Expected a GET handler");
        assert_eq!(get_route.unwrap().path, Some("/api/users".to_string()));
        assert_eq!(get_route.unwrap().framework, "nextjs");

        let post_route = route_patterns
            .iter()
            .find(|p| p.http_method == Some("POST".to_string()));
        assert!(post_route.is_some(), "Expected a POST handler");
        assert_eq!(post_route.unwrap().path, Some("/api/users".to_string()));
    }

    #[test]
    fn extracts_page() {
        let source = r#"
export default function DashboardPage() {
  return <main>Dashboard</main>
}
"#;
        let tree = parse_tsx(source);
        let patterns =
            extract_nextjs_patterns(tree.root_node(), source, "app/dashboard/page.tsx");

        let page = patterns
            .iter()
            .find(|p| p.kind == FrameworkPatternKind::FileRoute);
        assert!(page.is_some(), "Expected a FileRoute pattern for page.tsx");
        let p = page.unwrap();
        assert_eq!(p.name, Some("page".to_string()));
        assert_eq!(p.path, Some("/dashboard".to_string()));
        assert_eq!(p.framework, "nextjs");
    }

    #[test]
    fn extracts_dynamic_route() {
        let source = r#"
export async function GET(
  request: Request,
  { params }: { params: { id: string } }
) {
  return Response.json({ id: params.id })
}
"#;
        let tree = parse_ts(source);
        let patterns = extract_nextjs_patterns(
            tree.root_node(),
            source,
            "app/api/users/[id]/route.ts",
        );

        let routes: Vec<_> = patterns
            .iter()
            .filter(|p| p.kind == FrameworkPatternKind::Route)
            .collect();
        assert_eq!(routes.len(), 1, "Expected 1 route for dynamic segment");
        assert_eq!(routes[0].path, Some("/api/users/:id".to_string()));
        assert_eq!(routes[0].http_method, Some("GET".to_string()));
    }

    #[test]
    fn extracts_middleware() {
        let source = r#"
import { NextResponse } from 'next/server'
import type { NextRequest } from 'next/server'

export function middleware(request: NextRequest) {
  if (!request.cookies.has('token')) {
    return NextResponse.redirect(new URL('/login', request.url))
  }
  return NextResponse.next()
}

export const config = {
  matcher: ['/dashboard/:path*'],
}
"#;
        let tree = parse_ts(source);
        let patterns =
            extract_nextjs_patterns(tree.root_node(), source, "middleware.ts");

        let mw: Vec<_> = patterns
            .iter()
            .filter(|p| p.kind == FrameworkPatternKind::Middleware)
            .collect();
        assert_eq!(mw.len(), 1, "Expected 1 middleware pattern");
        assert_eq!(mw[0].framework, "nextjs");
        assert_eq!(mw[0].name, Some("middleware".to_string()));
    }

    #[test]
    fn extracts_error_boundary() {
        let source = r#"
'use client'

export default function ErrorBoundary({
  error,
  reset,
}: {
  error: Error & { digest?: string }
  reset: () => void
}) {
  return (
    <div>
      <h2>Something went wrong!</h2>
      <button onClick={() => reset()}>Try again</button>
    </div>
  )
}
"#;
        let tree = parse_tsx(source);
        let patterns =
            extract_nextjs_patterns(tree.root_node(), source, "app/dashboard/error.tsx");

        let error_handlers: Vec<_> = patterns
            .iter()
            .filter(|p| p.kind == FrameworkPatternKind::ErrorHandler)
            .collect();
        assert_eq!(error_handlers.len(), 1, "Expected 1 ErrorHandler pattern");
        let eh = &error_handlers[0];
        assert_eq!(eh.name, Some("error".to_string()));
        assert_eq!(eh.path, Some("/dashboard".to_string()));
        assert_eq!(eh.framework, "nextjs");
    }
}
