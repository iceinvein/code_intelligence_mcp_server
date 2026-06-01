import { Handle, Position, type NodeProps } from "@xyflow/react";
import type { SymbolNodeData } from "@/features/graph/toFlow";
import { cn } from "@/lib/utils";

export function SymbolNode({ data }: NodeProps) {
  const typedData = data as SymbolNodeData;
  const { node, isRoot } = typedData;
  return (
    <div
      className={cn(
        "rounded-md border bg-card px-2 py-1 font-mono text-[11px]",
        isRoot ? "border-primary" : "border-border",
      )}
      style={{ width: 200 }}
    >
      <Handle type="target" position={Position.Top} />
      <div className="flex items-baseline gap-2">
        <span className={cn("truncate", isRoot ? "text-primary" : "text-foreground")}>
          {node.name}
        </span>
        <span className="ml-auto text-[9px] uppercase tracking-wide text-muted-foreground">
          {node.kind}
        </span>
      </div>
      <div className="truncate text-[10px] text-muted-foreground">{node.file_path}</div>
      <Handle type="source" position={Position.Bottom} />
    </div>
  );
}
