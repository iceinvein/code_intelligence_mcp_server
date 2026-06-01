import { test, expect } from "bun:test";
import { extractCode } from "@/features/search/DefinitionPanel";

test("extractCode pulls the fenced code out of the markdown context", () => {
  const ctx =
    "\n## Definitions\n\n### lib.rs:1-3 `resolve_state` (function)\n```rust\npub fn resolve_state(x: i32) -> i32 {\n    x + 1\n}\n```\n\n";
  expect(extractCode(ctx)).toBe("pub fn resolve_state(x: i32) -> i32 {\n    x + 1\n}");
});

test("extractCode does not truncate when a symbol body contains inline triple backticks", () => {
  // A Rust raw string / doctest whose body contains ``` mid-line must not close the fence early.
  const ctx = '```rust\nlet s = "```hello```";\n```';
  expect(extractCode(ctx)).toBe('let s = "```hello```";');
});

test("extractCode joins multiple definition fences", () => {
  const ctx = "### a.rs `foo`\n```rust\nfn foo() {}\n```\n### b.rs `bar`\n```rust\nfn bar() {}\n```\n";
  expect(extractCode(ctx)).toBe("fn foo() {}\n\nfn bar() {}");
});

test("extractCode falls back to the trimmed context when there are no fences", () => {
  expect(extractCode("  no fences here  ")).toBe("no fences here");
});

test("extractCode recovers code from an unterminated trailing fence", () => {
  const ctx = "### a.rs `foo`\n```rust\nfn foo() {}"; // malformed: no closing ```
  expect(extractCode(ctx)).toBe("fn foo() {}");
});
