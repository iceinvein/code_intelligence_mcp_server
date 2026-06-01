import dagre from "@dagrejs/dagre";
import type { GraphEdge, GraphNode } from "@/api/graph";

export const NODE_WIDTH = 200;
export const NODE_HEIGHT = 44;

export type XY = { x: number; y: number };

/** Position nodes top-down with dagre. Returns top-left coords for React Flow. */
export function layoutGraph(nodes: GraphNode[], edges: GraphEdge[]): Map<string, XY> {
  const g = new dagre.graphlib.Graph();
  g.setGraph({ rankdir: "TB", nodesep: 40, ranksep: 60 });
  g.setDefaultEdgeLabel(() => ({}));

  for (const n of nodes) {
    g.setNode(n.id, { width: NODE_WIDTH, height: NODE_HEIGHT });
  }
  for (const e of edges) {
    if (g.hasNode(e.from) && g.hasNode(e.to)) {
      g.setEdge(e.from, e.to);
    }
  }

  dagre.layout(g);

  const out = new Map<string, XY>();
  for (const n of nodes) {
    const d = g.node(n.id);
    out.set(n.id, { x: d.x - NODE_WIDTH / 2, y: d.y - NODE_HEIGHT / 2 });
  }
  return out;
}
