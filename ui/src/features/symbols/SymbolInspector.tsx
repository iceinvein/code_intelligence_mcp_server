import { DefinitionPanel } from "@/features/search/DefinitionPanel";
import { UsageExamplesPanel } from "@/features/symbols/UsageExamplesPanel";

export function SymbolInspector({
  repoPath,
  symbolName,
  file,
}: {
  repoPath: string;
  symbolName: string;
  file: string;
}) {
  return (
    <div>
      <div className="border-b border-border pb-3">
        <div className="font-mono text-sm font-medium text-foreground">{symbolName}</div>
        <div className="mt-0.5 truncate font-mono text-[0.6875rem] text-muted-foreground">{file}</div>
      </div>
      <DefinitionPanel repoPath={repoPath} symbolName={symbolName} file={file} />
      <UsageExamplesPanel repoPath={repoPath} symbolName={symbolName} file={file} />
    </div>
  );
}
