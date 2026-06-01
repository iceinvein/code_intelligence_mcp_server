import type { SearchHit } from "@/api/search";

export function ResultRow({ hit }: { hit: SearchHit; repoPath: string }) {
  return (
    <div className="border-b border-border py-2">
      <div className="flex items-baseline gap-3">
        <span className="text-primary">{hit.symbol_name}</span>
        <span className="text-[11px] text-muted-foreground">{hit.kind}</span>
        {hit.exported ? <span className="text-[10px] text-muted-foreground">exported</span> : null}
        <span className="ml-auto font-mono text-[11px] text-muted-foreground">
          {hit.file_path} &middot; {hit.score.toFixed(3)}
        </span>
      </div>
    </div>
  );
}
