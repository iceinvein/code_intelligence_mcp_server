import { test, expect, mock, afterEach } from "bun:test";
import { searchCode, getDefinition, findReferences } from "@/api/search";

afterEach(() => {
  mock.restore();
});

function envelope(command: string, result: unknown) {
  return {
    ok: true,
    command,
    repo: { path: "/repo/demo", id: "abc" },
    index: { version_unix_s: 1, fresh: true },
    warnings: [],
    result,
  };
}

test("searchCode unwraps the envelope result and posts repo + query", async () => {
  const calls: Array<{ url: string; body: string | null }> = [];
  globalThis.fetch = mock(async (url: string, init?: RequestInit) => {
    calls.push({ url: String(url), body: (init?.body as string) ?? null });
    return new Response(
      JSON.stringify(envelope("search", { query: "x", limit: 25, hits: [{ id: "s", name: "foo", kind: "fn", file_path: "a.rs", score: 0.9 }], hits_budget: { returned_count: 1, total_count: 1, truncated: false } })),
      { status: 200 },
    );
  }) as unknown as typeof fetch;

  const data = await searchCode("/repo/demo", "x");
  expect(data.hits_budget.total_count).toBe(1);
  expect(data.hits[0]!.name).toBe("foo");
  expect(calls[0]!.url).toBe("/api/query/search");
  expect(JSON.parse(calls[0]!.body!)).toEqual({ repo: "/repo/demo", query: "x", limit: 25 });
});

test("getDefinition unwraps the definition envelope", async () => {
  globalThis.fetch = mock(async () =>
    new Response(JSON.stringify(envelope("definition", { symbol_name: "foo", count: 1, definitions: [{ id: "s", file_path: "a.rs", language: "rust", kind: "fn", name: "foo", exported: true, start_line: 1, end_line: 3 }], context: "fn foo() {}" })), { status: 200 }),
  ) as unknown as typeof fetch;

  const data = await getDefinition("/repo/demo", "foo", "a.rs");
  expect(data.context).toBe("fn foo() {}");
  expect(data.definitions[0]!.language).toBe("rust");
});

test("findReferences unwraps the references envelope and posts to the references path", async () => {
  const calls: Array<{ url: string; body: string | null }> = [];
  globalThis.fetch = mock(async (url: string, init?: RequestInit) => {
    calls.push({ url: String(url), body: (init?.body as string) ?? null });
    return new Response(
      JSON.stringify(envelope("references", { symbol_name: "foo", reference_type: "all", count: 1, references: [{ from_symbol_name: "bar", from_symbol_file: "b.rs", reference_type: "call", at_file: "b.rs", at_line: 10 }] })),
      { status: 200 },
    );
  }) as unknown as typeof fetch;

  const data = await findReferences("/repo/demo", "foo", "a.rs");
  expect(data.count).toBe(1);
  expect(data.references[0]!.at_file).toBe("b.rs");
  expect(calls[0]!.url).toBe("/api/query/references");
  expect(JSON.parse(calls[0]!.body!)).toEqual({ repo: "/repo/demo", symbol_name: "foo", file: "a.rs" });
});
