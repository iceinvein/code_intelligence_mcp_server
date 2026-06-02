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
    return <div className="text-[0.6875rem] text-muted-foreground">no symbols in this file</div>;
  }

  return (
    <div className="flex flex-col gap-0.5">
      {symbols.map((s) => (
        <button
          key={s.id}
          onClick={() => onSelect(s.name)}
          className={cn(
            "flex min-h-5 items-baseline gap-2 rounded px-1.5 py-1 text-left font-mono text-[0.6875rem] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring",
            selectedName === s.name
              ? "bg-primary/10 text-primary"
              : "text-foreground hover:bg-muted hover:text-foreground",
          )}
        >
          <span className="truncate">{s.name}</span>
          <span className="ml-auto text-[0.625rem] uppercase tracking-[0.08em] text-muted-foreground">
            {s.kind}
          </span>
        </button>
      ))}
    </div>
  );
}
