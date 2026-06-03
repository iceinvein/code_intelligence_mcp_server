import { useState } from "react";
import { ChevronRight } from "lucide-react";
import { cn } from "@/lib/utils";
import type { SearchHit } from "@/api/search";
import { DefinitionPanel } from "@/features/search/DefinitionPanel";

export function ResultRow({ hit, repoPath }: { hit: SearchHit; repoPath: string }) {
  const [expanded, setExpanded] = useState(false);
  return (
    <div>
      <button
        className="flex w-full items-baseline gap-3 px-3.5 py-2.5 text-left transition-colors duration-150 hover:bg-muted/60"
        onClick={() => setExpanded((v) => !v)}
        aria-expanded={expanded}
      >
        <ChevronRight
          className={cn(
            "h-3.5 w-3.5 shrink-0 translate-y-0.5 text-muted-foreground transition-transform duration-150",
            expanded && "rotate-90",
          )}
          aria-hidden
        />
        <span
          className={cn(
            "min-w-0 shrink truncate font-medium",
            expanded ? "text-primary" : "text-foreground",
          )}
        >
          {hit.name}
        </span>
        <span className="shrink-0 text-[0.6875rem] text-muted-foreground">{hit.kind}</span>
        <span className="ml-auto min-w-0 truncate font-mono text-[0.6875rem] text-muted-foreground">
          {hit.file_path}
        </span>
        <span className="shrink-0 font-mono text-[0.6875rem] tabular-nums text-muted-foreground">
          {hit.score.toFixed(3)}
        </span>
      </button>
      {expanded ? (
        <div className="px-3.5 pb-3 pl-10">
          <DefinitionPanel repoPath={repoPath} symbolName={hit.name} file={hit.file_path} />
        </div>
      ) : null}
    </div>
  );
}
