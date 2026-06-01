import { useEffect, useMemo, useRef, useState } from "react";
import { ChevronDown, ChevronRight } from "lucide-react";
import { useVirtualizer } from "@tanstack/react-virtual";
import type { IndexedFile } from "@/api/symbols";
import { allDirPaths, buildTree, flattenVisible, type FlatRow } from "@/features/symbols/tree";
import { cn } from "@/lib/utils";

const ROW_HEIGHT = 22;

function Row({
  row,
  selectedFile,
  onSelectFile,
  onToggle,
}: {
  row: FlatRow;
  selectedFile: string | null;
  onSelectFile: (path: string) => void;
  onToggle: (path: string) => void;
}) {
  const pad = { paddingLeft: `${row.depth * 12}px` };

  if (row.type === "file") {
    return (
      <button
        style={pad}
        onClick={() => onSelectFile(row.path)}
        className={cn(
          "flex h-full w-full items-center gap-2 text-left font-mono text-[11px] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring",
          selectedFile === row.path ? "text-primary" : "text-foreground hover:text-primary",
        )}
      >
        <span className="truncate">{row.name}</span>
        <span className="ml-auto pr-1 text-muted-foreground">{row.symbolCount}</span>
      </button>
    );
  }

  const Icon = row.open ? ChevronDown : ChevronRight;
  return (
    <button
      style={pad}
      onClick={() => onToggle(row.path)}
      className="flex h-full w-full items-center gap-1 text-left font-mono text-[11px] text-muted-foreground hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
    >
      <Icon className="h-3 w-3 shrink-0" aria-hidden="true" />
      <span className="truncate">{row.name}/</span>
    </button>
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
  const [expanded, setExpanded] = useState<Set<string>>(() => new Set(allDirPaths(tree)));

  // Reset to all-expanded when the file set changes (e.g. switching repos).
  useEffect(() => {
    setExpanded(new Set(allDirPaths(tree)));
  }, [tree]);

  const rows = useMemo(() => flattenVisible(tree, expanded), [tree, expanded]);

  const parentRef = useRef<HTMLDivElement>(null);
  const virtualizer = useVirtualizer({
    count: rows.length,
    getScrollElement: () => parentRef.current,
    estimateSize: () => ROW_HEIGHT,
    overscan: 12,
  });

  const toggle = (path: string) =>
    setExpanded((prev) => {
      const next = new Set(prev);
      if (next.has(path)) next.delete(path);
      else next.add(path);
      return next;
    });

  return (
    <div ref={parentRef} className="h-[70vh] overflow-auto">
      <div style={{ height: `${virtualizer.getTotalSize()}px`, position: "relative" }}>
        {virtualizer.getVirtualItems().map((item) => {
          const row = rows[item.index]!;
          return (
            <div
              key={row.path}
              className="absolute left-0 top-0 w-full"
              style={{ height: `${ROW_HEIGHT}px`, transform: `translateY(${item.start}px)` }}
            >
              <Row
                row={row}
                selectedFile={selectedFile}
                onSelectFile={onSelectFile}
                onToggle={toggle}
              />
            </div>
          );
        })}
      </div>
    </div>
  );
}
