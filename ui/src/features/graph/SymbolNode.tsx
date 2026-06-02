import { Handle, Position, type NodeProps } from "@xyflow/react";
import type { SymbolNodeData } from "@/features/graph/toFlow";
import { cn } from "@/lib/utils";

export function SymbolNode({ data }: NodeProps) {
  const typedData = data as SymbolNodeData;
  const { node, isRoot } = typedData;
  return (
    <div
      className={cn(
        "rounded-md border bg-card px-2.5 py-1.5 font-mono text-[0.6875rem] shadow-sm",
        isRoot ? "border-primary ring-1 ring-primary/30" : "border-border",
      )}
      style={{ width: 200 }}
    >
      <Handle type="target" position={Position.Top} />
      <div className="flex items-baseline gap-2">
        <span className={cn("truncate font-medium", isRoot ? "text-primary" : "text-foreground")}>
          {node.name}
        </span>
        <span className="ml-auto text-[0.5625rem] uppercase tracking-[0.08em] text-muted-foreground">
          {node.kind}
        </span>
      </div>
      <div className="truncate text-[0.625rem] text-muted-foreground">{node.file_path}</div>
      <Handle type="source" position={Position.Bottom} />
    </div>
  );
}
