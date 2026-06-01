import { afterEach, expect, mock, test } from "bun:test";
import { fetchFiles, fetchFileSymbols, fetchUsageExamples } from "@/api/symbols";

afterEach(() => {
  mock.restore();
});

function envelope(result: unknown) {
  return new Response(
    JSON.stringify({
      ok: true,
      command: "x",
      repo: { path: "/r", id: "id" },
      index: { version_unix_s: 1, fresh: true },
      warnings: [],
      result,
    }),
    { status: 200 },
  );
}

test("fetchFiles posts repo and unwraps the files array", async () => {
  const calls: Array<{ url: string; body: string | null }> = [];
  globalThis.fetch = mock(async (url: string, init?: RequestInit) => {
    calls.push({ url: String(url), body: (init?.body as string) ?? null });
    return envelope({ files: [{ path: "a.rs", symbol_count: 3 }] });
  }) as unknown as typeof fetch;

  const data = await fetchFiles("/r");
  expect(calls[0]!.url).toBe("/api/query/files");
  expect(JSON.parse(calls[0]!.body!)).toEqual({ repo: "/r" });
  expect(data.files[0]!.path).toBe("a.rs");
  expect(data.files[0]!.symbol_count).toBe(3);
});

test("fetchFileSymbols posts repo + file_path and unwraps symbols", async () => {
  globalThis.fetch = mock(async () =>
    envelope({
      file_path: "a.rs",
      count: 1,
      symbols: [
        {
          id: "s1",
          name: "foo",
          kind: "function",
          language: "rust",
          exported: true,
          start_byte: 0,
          end_byte: 1,
          start_line: 1,
          end_line: 2,
        },
      ],
    }),
  ) as unknown as typeof fetch;

  const data = await fetchFileSymbols("/r", "a.rs");
  expect(data.symbols[0]!.name).toBe("foo");
});

test("fetchUsageExamples posts repo + symbol_name and unwraps examples", async () => {
  globalThis.fetch = mock(async () =>
    envelope({
      symbol_name: "foo",
      count: 1,
      examples: [
        {
          reference_type: "call",
          from_file_path: "b.rs",
          from_symbol_name: "bar",
          at_file: "b.rs",
          at_line: 9,
          snippet: "foo()",
        },
      ],
    }),
  ) as unknown as typeof fetch;

  const data = await fetchUsageExamples("/r", "foo");
  expect(data.examples[0]!.from_symbol_name).toBe("bar");
  expect(data.examples[0]!.at_line).toBe(9);
});
