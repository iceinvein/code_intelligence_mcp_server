import type { Edge, Node } from "@xyflow/react";
import type { GraphData, GraphNode } from "@/api/graph";
import type { XY } from "@/features/graph/layout";

export type SymbolNodeData = { node: GraphNode; isRoot: boolean } & Record<string, unknown>;
export type SymbolFlowNode = Node<SymbolNodeData, "symbol">;

export function toFlow(
  data: GraphData,
  positions: Map<string, XY>,
  rootId: string | null,
): { nodes: SymbolFlowNode[]; edges: Edge[] } {
  const nodes: SymbolFlowNode[] = data.nodes.map((n) => ({
    id: n.id,
    type: "symbol",
    position: positions.get(n.id) ?? { x: 0, y: 0 },
    data: { node: n, isRoot: n.id === rootId },
  }));

  const edges: Edge[] = data.edges
    .filter((e) => positions.has(e.from) && positions.has(e.to))
    .map((e) => ({
      id: `${e.from}->${e.to}-${e.edge_type}`,
      source: e.from,
      target: e.to,
      label: e.edge_type,
    }));

  return { nodes, edges };
}
