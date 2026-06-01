import { expect, test } from "bun:test";
import type { GraphEdge, GraphNode } from "@/api/graph";
import { layoutGraph } from "@/features/graph/layout";

const node = (id: string): GraphNode => ({
  id,
  name: id,
  kind: "function",
  file_path: "a.rs",
  exported: true,
  line_range: [1, 2],
});

test("layoutGraph positions every node and stacks child below parent", () => {
  const nodes: GraphNode[] = [node("a"), node("b")];
  const edges: GraphEdge[] = [
    {
      from: "a",
      to: "b",
      edge_type: "call",
      at_file: "a.rs",
      at_line: 1,
      evidence_count: 1,
      resolution: "exact",
    },
  ];
  const pos = layoutGraph(nodes, edges);
  expect(pos.get("a")).toBeDefined();
  expect(pos.get("b")).toBeDefined();
  expect(pos.get("b")!.y).toBeGreaterThan(pos.get("a")!.y);
});
