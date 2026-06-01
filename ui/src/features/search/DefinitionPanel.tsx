import { useDefinition, useReferences } from "@/features/search/useSearch";
import { CodeBlock } from "@/components/ui/code-block";
import type { ReferenceEdge } from "@/api/search";

// The definition `context` returned by the API is a markdown document (headers plus fenced
// code blocks), not raw source. Pull the fenced code out so shiki highlights code, not markdown.
// Fences are matched line-by-line (a line that starts with ```), so triple backticks appearing
// mid-line inside a symbol body (e.g. a Rust raw string or doctest) do not prematurely close it.
export function extractCode(context: string): string {
  const blocks: string[] = [];
  let current: string[] | null = null;
  for (const line of context.split("\n")) {
    if (line.startsWith("```")) {
      if (current === null) {
        current = [];
      } else {
        blocks.push(current.join("\n"));
        current = null;
      }
      continue;
    }
    if (current !== null) current.push(line);
  }
  // Tolerate an unterminated trailing fence (malformed markdown) by keeping what we collected.
  if (current !== null && current.length > 0) blocks.push(current.join("\n"));
  return blocks.length > 0 ? blocks.join("\n\n") : context.trim();
}

function groupByFile(refs: ReferenceEdge[]): Array<{ file: string; rows: ReferenceEdge[] }> {
  const map = new Map<string, ReferenceEdge[]>();
  for (const r of refs) {
    const list = map.get(r.at_file) ?? [];
    list.push(r);
    map.set(r.at_file, list);
  }
  return Array.from(map.entries())
    .sort((a, b) => a[0].localeCompare(b[0]))
    .map(([file, rows]) => ({ file, rows }));
}

export function DefinitionPanel({ repoPath, symbolName, file }: { repoPath: string; symbolName: string; file: string }) {
  const def = useDefinition(repoPath, symbolName, file, true);
  const refs = useReferences(repoPath, symbolName, file, true);

  const lang = def.data?.definitions[0]?.language ?? "text";
  const code = extractCode(def.data?.context ?? "");
  const groups = groupByFile(refs.data?.references ?? []);

  return (
    <div className="mt-2 border-t border-border pt-2">
      {def.isLoading ? (
        <div className="text-[11px] text-muted-foreground">loading definition...</div>
      ) : def.isError ? (
        <div className="text-[11px] text-destructive">failed to load definition</div>
      ) : code ? (
        <CodeBlock code={code} lang={lang} />
      ) : (
        <div className="text-[11px] text-muted-foreground">no definition found</div>
      )}

      <div className="mt-2 text-[10px] uppercase tracking-[0.18em] text-label">
        references &middot; {refs.data?.count ?? 0}
      </div>
      {refs.isLoading ? (
        <div className="text-[11px] text-muted-foreground">loading references...</div>
      ) : refs.isError ? (
        <div className="text-[11px] text-destructive">failed to load references</div>
      ) : groups.length === 0 ? (
        <div className="text-[11px] text-muted-foreground">no references</div>
      ) : (
        <div className="flex flex-col gap-2 font-mono text-[11px]">
          {groups.map((g) => (
            <div key={g.file}>
              <div className="text-muted-foreground">{g.file}</div>
              {g.rows.map((r, i) => (
                <div key={`${r.at_line}-${i}`} className="pl-3 text-foreground">
                  <span className="text-muted-foreground">L{r.at_line}</span> {r.from_symbol_name}{" "}
                  <span className="text-muted-foreground">({r.reference_type})</span>
                </div>
              ))}
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
