import { useMemo } from "react";
import { Background, Controls, MiniMap, ReactFlow } from "@xyflow/react";
import "@xyflow/react/dist/style.css";
import type { GraphData } from "@/api/graph";
import { useResolvedTheme } from "@/lib/theme";
import { layoutGraph } from "@/features/graph/layout";
import { SymbolNode } from "@/features/graph/SymbolNode";
import { toFlow, type SymbolFlowNode, type SymbolNodeData } from "@/features/graph/toFlow";

const nodeTypes = { symbol: SymbolNode };

export function GraphCanvas({
  data,
  rootId,
  onSelect,
  onReRoot,
}: {
  data: GraphData;
  rootId: string | null;
  onSelect: (node: SymbolNodeData["node"]) => void;
  onReRoot: (node: SymbolNodeData["node"]) => void;
}) {
  const colorMode = useResolvedTheme();
  const { nodes, edges } = useMemo(() => {
    const positions = layoutGraph(data.nodes, data.edges);
    return toFlow(data, positions, rootId);
  }, [data, rootId]);

  return (
    <div className="h-[calc(100vh-9rem)] w-full">
      <ReactFlow<SymbolFlowNode>
        nodes={nodes}
        edges={edges}
        nodeTypes={nodeTypes}
        colorMode={colorMode}
        fitView
        onNodeClick={(_e, n) => onSelect(n.data.node)}
        onNodeDoubleClick={(_e, n) => onReRoot(n.data.node)}
      >
        <Background />
        <Controls />
        <MiniMap pannable zoomable />
      </ReactFlow>
    </div>
  );
}
