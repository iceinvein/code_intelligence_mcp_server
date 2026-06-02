import { useUsageExamples } from "@/features/symbols/useSymbols";
import { SectionLabel } from "@/components/ui/datasheet";

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
    <div className="mt-3 border-t border-border pt-3">
      <SectionLabel as="h3" className="mb-2" count={examples.data?.count ?? 0}>
        usage examples
      </SectionLabel>
      {examples.isLoading ? (
        <div className="text-[0.6875rem] text-muted-foreground">loading usage examples…</div>
      ) : examples.isError ? (
        <div className="text-[0.6875rem] text-destructive">failed to load usage examples</div>
      ) : rows.length === 0 ? (
        <div className="text-[0.6875rem] text-muted-foreground">no usage examples</div>
      ) : (
        <div className="flex flex-col gap-2 font-mono text-[0.6875rem]">
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
