import { useMemo, useState } from "react";
import { ChevronDown, ChevronRight } from "lucide-react";
import type { IndexedFile } from "@/api/symbols";
import { buildTree, type TreeNode } from "@/features/symbols/tree";
import { cn } from "@/lib/utils";

function Node({
  node,
  depth,
  selectedFile,
  onSelectFile,
}: {
  node: TreeNode;
  depth: number;
  selectedFile: string | null;
  onSelectFile: (path: string) => void;
}) {
  const [open, setOpen] = useState(true);
  const pad = { paddingLeft: `${depth * 12}px` };

  if (node.type === "file") {
    return (
      <button
        style={pad}
        onClick={() => onSelectFile(node.path)}
        className={cn(
          "flex min-h-5 w-full items-baseline gap-2 py-0.5 text-left font-mono text-[11px] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring",
          selectedFile === node.path ? "text-primary" : "text-foreground hover:text-primary",
        )}
      >
        <span className="truncate">{node.name}</span>
        <span className="ml-auto pr-1 text-muted-foreground">{node.symbolCount}</span>
      </button>
    );
  }

  const Icon = open ? ChevronDown : ChevronRight;

  return (
    <div>
      <button
        style={pad}
        onClick={() => setOpen((v) => !v)}
        className="flex min-h-5 w-full items-center gap-1 py-0.5 text-left font-mono text-[11px] text-muted-foreground hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
      >
        <Icon className="h-3 w-3 shrink-0" aria-hidden="true" />
        <span className="truncate">{node.name}/</span>
      </button>
      {open
        ? node.children.map((child) => (
            <Node
              key={child.path}
              node={child}
              depth={depth + 1}
              selectedFile={selectedFile}
              onSelectFile={onSelectFile}
            />
          ))
        : null}
    </div>
  );
}

export function FileTree({
  files,
  selectedFile,
  onSelectFile,
}: {
  files: IndexedFile[];
  selectedFile: string | null;
  onSelectFile: (path: string) => void;
}) {
  const tree = useMemo(() => buildTree(files), [files]);
  return (
    <div>
      {tree.map((node) => (
        <Node
          key={node.path}
          node={node}
          depth={0}
          selectedFile={selectedFile}
          onSelectFile={onSelectFile}
        />
      ))}
    </div>
  );
}
