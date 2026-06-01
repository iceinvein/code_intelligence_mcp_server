import { expect, test } from "bun:test";
import type { GraphData } from "@/api/graph";
import { toFlow } from "@/features/graph/toFlow";

test("toFlow maps nodes/edges and preserves ids", () => {
  const data: GraphData = {
    symbol_name: "a",
    direction: "callees",
    depth: 2,
    nodes: [
      { id: "a", name: "a", kind: "function", file_path: "a.rs", exported: true, line_range: [1, 2] },
      { id: "b", name: "b", kind: "function", file_path: "b.rs", exported: false, line_range: [3, 4] },
    ],
    edges: [
      {
        from: "a",
        to: "b",
        edge_type: "call",
        at_file: "a.rs",
        at_line: 1,
        evidence_count: 1,
        resolution: "exact",
      },
    ],
  };
  const positions = new Map([
    ["a", { x: 0, y: 0 }],
    ["b", { x: 0, y: 100 }],
  ]);

  const { nodes, edges } = toFlow(data, positions, "a");
  expect(nodes.map((n) => n.id)).toEqual(["a", "b"]);
  expect(nodes[0]!.position).toEqual({ x: 0, y: 0 });
  expect(nodes[0]!.data.isRoot).toBe(true);
  expect(nodes[1]!.data.isRoot).toBe(false);
  expect(edges[0]!.source).toBe("a");
  expect(edges[0]!.target).toBe("b");
  expect(edges[0]!.id).toBe("a->b-call");
});
