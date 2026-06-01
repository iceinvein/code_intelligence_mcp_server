import { useUsageExamples } from "@/features/symbols/useSymbols";

export function UsageExamplesPanel({
  repoPath,
  symbolName,
  file,
}: {
  repoPath: string;
  symbolName: string;
  file: string;
}) {
  const examples = useUsageExamples(repoPath, symbolName, file, true);
  const rows = examples.data?.examples ?? [];

  return (
    <div className="mt-2 border-t border-border pt-2">
      <div className="mb-1 text-[10px] uppercase tracking-[0.18em] text-label">
        usage examples &middot; {examples.data?.count ?? 0}
      </div>
      {examples.isLoading ? (
        <div className="text-[11px] text-muted-foreground">loading usage examples...</div>
      ) : examples.isError ? (
        <div className="text-[11px] text-destructive">failed to load usage examples</div>
      ) : rows.length === 0 ? (
        <div className="text-[11px] text-muted-foreground">no usage examples</div>
      ) : (
        <div className="flex flex-col gap-1 font-mono text-[11px]">
          {rows.map((ex, i) => (
            <div key={`${ex.at_file}-${ex.at_line}-${i}`}>
              <div className="text-muted-foreground">
                {ex.at_file}
                <span className="text-muted-foreground"> L{ex.at_line}</span>
                <span className="text-muted-foreground"> ({ex.reference_type})</span>
              </div>
              <div className="pl-3 text-foreground">{ex.snippet}</div>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
