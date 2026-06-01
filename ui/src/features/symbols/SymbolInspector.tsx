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
      <div className="mb-1 font-mono text-[12px] text-primary">{symbolName}</div>
      <div className="text-[11px] text-muted-foreground">{file}</div>
      <DefinitionPanel repoPath={repoPath} symbolName={symbolName} file={file} />
      <UsageExamplesPanel repoPath={repoPath} symbolName={symbolName} file={file} />
    </div>
  );
}
