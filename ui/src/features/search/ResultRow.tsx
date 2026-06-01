import { useState } from "react";
import type { SearchHit } from "@/api/search";
import { DefinitionPanel } from "@/features/search/DefinitionPanel";

export function ResultRow({ hit, repoPath }: { hit: SearchHit; repoPath: string }) {
  const [expanded, setExpanded] = useState(false);
  return (
    <div className="border-b border-border py-2">
      <button
        className="flex w-full items-baseline gap-3 text-left"
        onClick={() => setExpanded((v) => !v)}
        aria-expanded={expanded}
      >
        <span className="text-primary">{hit.name}</span>
        <span className="text-[11px] text-muted-foreground">{hit.kind}</span>
        <span className="ml-auto font-mono text-[11px] text-muted-foreground">
          {hit.file_path} &middot; {hit.score.toFixed(3)}
        </span>
      </button>
      {expanded ? <DefinitionPanel repoPath={repoPath} symbolName={hit.name} file={hit.file_path} /> : null}
    </div>
  );
}
