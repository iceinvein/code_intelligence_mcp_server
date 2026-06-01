import type React from "react";
import { test, expect, mock, afterEach } from "bun:test";
import { render, fireEvent, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";

mock.module("@/lib/shiki", () => ({
  highlight: async (code: string) => `<pre class="shiki"><code>${code}</code></pre>`,
}));

afterEach(() => {
  mock.restore();
});

// References are returned out of alphabetical order (b.rs before a_alpha.rs) so the
// test verifies groupByFile sorts them, not just that it groups them.
function mockSearchFetch() {
  globalThis.fetch = mock(async (url: string) => {
    const u = String(url);
    if (u.endsWith("/api/query/definition")) {
      return new Response(JSON.stringify({ ok: true, command: "definition", repo: { path: "/r", id: "a" }, index: { version_unix_s: 1, fresh: true }, warnings: [], result: { symbol_name: "foo", count: 1, definitions: [{ id: "s", file_path: "a.rs", language: "rust", kind: "fn", name: "foo", exported: true, start_line: 1, end_line: 2 }], context: "## Definitions\n\n### a.rs:1-2 `foo` (function)\n```rust\nfn foo() {}\n```\n" } }), { status: 200 });
    }
    return new Response(JSON.stringify({ ok: true, command: "references", repo: { path: "/r", id: "a" }, index: { version_unix_s: 1, fresh: true }, warnings: [], result: { symbol_name: "foo", reference_type: "all", count: 2, references: [{ from_symbol_name: "bar", from_symbol_file: "b.rs", reference_type: "call", at_file: "b.rs", at_line: 10 }, { from_symbol_name: "baz", from_symbol_file: "a_alpha.rs", reference_type: "call", at_file: "a_alpha.rs", at_line: 20 }] } }), { status: 200 });
  }) as unknown as typeof fetch;
}

function renderRow(ui: React.ReactElement) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={client}>{ui}</QueryClientProvider>);
}

test("expanding a result loads its definition and references grouped and sorted by file", async () => {
  mockSearchFetch();

  const { ResultRow } = await import("@/features/search/ResultRow");
  const hit = { id: "s1", name: "foo", kind: "fn", file_path: "a.rs", score: 0.9 };
  const { findByText, getByRole, container } = renderRow(<ResultRow hit={hit} repoPath="/r" />);

  fireEvent.click(getByRole("button"));
  expect(await findByText(/fn foo/)).toBeDefined();
  // Both file groups render, with their reference rows.
  expect(await findByText("b.rs")).toBeDefined();
  expect(await findByText("a_alpha.rs")).toBeDefined();
  expect(await findByText(/bar/)).toBeDefined();
  expect(await findByText(/baz/)).toBeDefined();

  // a_alpha.rs sorts before b.rs even though it was returned second.
  const text = container.textContent ?? "";
  expect(text.indexOf("a_alpha.rs")).toBeLessThan(text.indexOf("b.rs"));
});

test("collapsing a result hides the definition panel again", async () => {
  mockSearchFetch();

  const { ResultRow } = await import("@/features/search/ResultRow");
  const hit = { id: "s1", name: "foo", kind: "fn", file_path: "a.rs", score: 0.9 };
  const { findByText, queryByText, getByRole } = renderRow(<ResultRow hit={hit} repoPath="/r" />);

  fireEvent.click(getByRole("button"));
  expect(await findByText("b.rs")).toBeDefined();

  fireEvent.click(getByRole("button"));
  await waitFor(() => expect(queryByText("b.rs")).toBeNull());
});
