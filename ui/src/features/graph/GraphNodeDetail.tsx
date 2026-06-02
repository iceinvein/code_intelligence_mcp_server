import { Link } from "react-router";
import type { GraphNode } from "@/api/graph";
import { Button } from "@/components/ui/button";
import { SymbolInspector } from "@/features/symbols/SymbolInspector";

export function GraphNodeDetail({
  repoPath,
  repoId,
  node,
  onReRoot,
}: {
  repoPath: string;
  repoId: string;
  node: GraphNode;
  onReRoot: (node: GraphNode) => void;
}) {
  return (
    <div className="flex flex-col gap-2">
      <div className="flex gap-2">
        <Button size="sm" onClick={() => onReRoot(node)}>
          re-root here
        </Button>
        <Button
          render={
            <Link
              to={`/symbols?repo=${encodeURIComponent(repoId)}&file=${encodeURIComponent(
                node.file_path,
              )}&sym=${encodeURIComponent(node.name)}`}
            />
          }
          size="sm"
          variant="outline"
        >
          open in symbols
        </Button>
      </div>
      <SymbolInspector repoPath={repoPath} symbolName={node.name} file={node.file_path} />
    </div>
  );
}
