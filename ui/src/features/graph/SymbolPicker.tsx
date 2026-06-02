import { useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { searchCode } from "@/api/search";

export function SymbolPicker({
  repoPath,
  onPick,
}: {
  repoPath: string;
  onPick: (name: string, file: string) => void;
}) {
  const [draft, setDraft] = useState("");
  const [query, setQuery] = useState("");

  const results = useQuery({
    queryKey: ["graph-picker", repoPath, query],
    queryFn: ({ signal }) => searchCode(repoPath, query, 12, signal),
    enabled: query.trim().length > 0,
    staleTime: 30_000,
  });

  const hits = results.data?.hits ?? [];

  return (
    <div className="relative">
      <input
        value={draft}
        onChange={(e) => setDraft(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter") setQuery(draft.trim());
        }}
        placeholder="find a symbol…"
        aria-label="symbol search"
        className="h-8 w-72 rounded-md border border-input bg-card px-2.5 font-mono text-sm outline-none placeholder:text-muted-foreground focus-visible:ring-2 focus-visible:ring-ring"
      />
      {query && hits.length > 0 ? (
        <div className="absolute z-10 mt-1 max-h-64 w-72 overflow-auto rounded-md border border-border bg-popover shadow-[0_12px_40px_-16px_oklch(20%_0.02_250/0.45)]">
          {hits.map((h) => (
            <button
              key={h.id}
              onClick={() => {
                onPick(h.name, h.file_path);
                setQuery("");
                setDraft(h.name);
              }}
              className="flex w-full items-baseline gap-2 px-2.5 py-1.5 text-left font-mono text-[0.6875rem] hover:bg-muted"
            >
              <span className="font-medium text-foreground">{h.name}</span>
              <span className="text-muted-foreground">{h.kind}</span>
              <span className="ml-auto truncate text-muted-foreground">{h.file_path}</span>
            </button>
          ))}
        </div>
      ) : null}
    </div>
  );
}
