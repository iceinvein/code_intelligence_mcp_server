import type { FileSymbol } from "@/api/symbols";
import { cn } from "@/lib/utils";

export function SymbolOutline({
  symbols,
  selectedName,
  onSelect,
}: {
  symbols: FileSymbol[];
  selectedName: string | null;
  onSelect: (name: string) => void;
}) {
  if (symbols.length === 0) {
    return <div className="text-[11px] text-muted-foreground">no symbols in this file</div>;
  }

  return (
    <div className="flex flex-col">
      {symbols.map((s) => (
        <button
          key={s.id}
          onClick={() => onSelect(s.name)}
          className={cn(
            "flex min-h-5 items-baseline gap-2 py-0.5 text-left font-mono text-[11px] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring",
            selectedName === s.name ? "text-primary" : "text-foreground hover:text-primary",
          )}
        >
          <span className="truncate">{s.name}</span>
          <span className="ml-auto text-[10px] uppercase tracking-wide text-muted-foreground">
            {s.kind}
          </span>
        </button>
      ))}
    </div>
  );
}
