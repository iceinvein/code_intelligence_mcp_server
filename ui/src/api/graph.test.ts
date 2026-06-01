import { afterEach, expect, mock, test } from "bun:test";
import { fetchCallHierarchy } from "@/api/graph";

afterEach(() => {
  mock.restore();
});

test("fetchCallHierarchy posts the params and unwraps nodes/edges", async () => {
  const calls: Array<{ url: string; body: string | null }> = [];
  globalThis.fetch = mock(async (url: string, init?: RequestInit) => {
    calls.push({ url: String(url), body: (init?.body as string) ?? null });
    return new Response(
      JSON.stringify({
        ok: true,
        command: "call-hierarchy",
        repo: { path: "/r", id: "id" },
        index: { version_unix_s: 1, fresh: true },
        warnings: [],
        result: {
          symbol_name: "foo",
          direction: "callees",
          depth: 2,
          nodes: [
            {
              id: "foo",
              name: "foo",
              kind: "function",
              file_path: "a.rs",
              exported: true,
              line_range: [1, 3],
            },
          ],
          edges: [
            {
              from: "foo",
              to: "bar",
              edge_type: "call",
              at_file: "a.rs",
              at_line: 2,
              evidence_count: 1,
              resolution: "exact",
            },
          ],
        },
      }),
      { status: 200 },
    );
  }) as unknown as typeof fetch;

  const data = await fetchCallHierarchy("/r", "foo", "a.rs", "callees", 2);
  expect(calls[0]!.url).toBe("/api/query/call-hierarchy");
  expect(JSON.parse(calls[0]!.body!)).toEqual({
    repo: "/r",
    symbol_name: "foo",
    file: "a.rs",
    direction: "callees",
    depth: 2,
  });
  expect(data.nodes[0]!.id).toBe("foo");
  expect(data.edges[0]!.edge_type).toBe("call");
});
