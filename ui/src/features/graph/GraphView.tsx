import { useEffect } from "react";
import { useSearchParams } from "react-router";
import type { GraphNode, GraphType } from "@/api/graph";
import { GraphCanvas } from "@/features/graph/GraphCanvas";
import { GraphNodeDetail } from "@/features/graph/GraphNodeDetail";
import { SymbolPicker } from "@/features/graph/SymbolPicker";
import { useGraph } from "@/features/graph/useGraph";
import { useRepos } from "@/features/repos/useRepos";

const TYPES: { key: GraphType; label: string }[] = [
  { key: "call", label: "call" },
  { key: "type", label: "type" },
  { key: "dependency", label: "dependency" },
];

const DIRECTIONS: Record<GraphType, string[]> = {
  call: ["callees", "callers", "both"],
  type: ["both", "downstream", "upstream"],
  dependency: ["downstream", "upstream", "both"],
};

export function GraphView() {
  const [params, setParams] = useSearchParams();
  const repos = useRepos().data?.repos ?? [];

  const repoId = params.get("repo") ?? "";
  const type = (params.get("type") as GraphType) || "call";
  const symbol = params.get("symbol");
  const file = params.get("file");
  const direction = params.get("dir") || DIRECTIONS[type][0]!;
  const depth = Number(params.get("depth") || "2");

  useEffect(() => {
    if (!repoId && repos.length === 1) {
      setParams(
        (prev) => {
          const next = new URLSearchParams(prev);
          next.set("repo", repos[0]!.id);
          return next;
        },
        { replace: true },
      );
    }
  }, [repoId, repos, setParams]);

  const selectedRepo = repos.find((r) => r.id === repoId) ?? null;
  const repoPath = selectedRepo?.path ?? null;

  const graph = useGraph(type, repoPath, symbol, file, direction, depth);

  const update = (patch: Record<string, string | null>) => {
    const next = new URLSearchParams(params);
    for (const [k, v] of Object.entries(patch)) {
      if (v === null) next.delete(k);
      else next.set(k, v);
    }
    setParams(next);
  };

  const selectedNode = (() => {
    const id = params.get("sel");
    return graph.data?.nodes.find((n) => n.id === id) ?? null;
  })();

  const reRoot = (node: GraphNode) => update({ symbol: node.name, file: node.file_path, sel: null });

  return (
    <section>
      <div className="mb-3 flex flex-wrap items-center gap-2">
        <h2 className="text-[10px] uppercase tracking-[0.18em] text-label">graph</h2>
        {repoPath ? (
          <SymbolPicker
            repoPath={repoPath}
            onPick={(name, f) => update({ symbol: name, file: f, sel: null })}
          />
        ) : null}
        <div className="flex gap-1">
          {TYPES.map((t) => (
            <button
              key={t.key}
              onClick={() => update({ type: t.key, dir: DIRECTIONS[t.key][0]! })}
              className={`border px-2 py-1 text-[11px] ${
                type === t.key ? "border-primary text-primary" : "border-border text-muted-foreground"
              }`}
            >
              {t.label}
            </button>
          ))}
        </div>
        <select
          value={direction}
          onChange={(e) => update({ dir: e.target.value })}
          className="h-7 rounded-md border border-border bg-card px-2 text-[12px]"
        >
          {DIRECTIONS[type].map((d) => (
            <option key={d} value={d}>
              {d}
            </option>
          ))}
        </select>
        <select
          value={String(depth)}
          onChange={(e) => update({ depth: e.target.value })}
          className="h-7 rounded-md border border-border bg-card px-2 text-[12px]"
        >
          {[1, 2, 3].map((d) => (
            <option key={d} value={d}>
              depth {d}
            </option>
          ))}
        </select>
        <select
          value={repoId}
          aria-label="repository"
          onChange={(e) => update({ repo: e.target.value })}
          className="ml-auto h-7 rounded-md border border-border bg-card px-2 text-[12px]"
        >
          <option value="">select repo</option>
          {repos.map((r) => (
            <option key={r.id} value={r.id}>
              {r.name}
            </option>
          ))}
        </select>
      </div>

      {!repoPath ? (
        <div className="text-xs text-muted-foreground">select a repository</div>
      ) : !symbol ? (
        <div className="text-xs text-muted-foreground">search for a symbol to root the graph</div>
      ) : graph.isLoading ? (
        <div className="text-xs text-muted-foreground">building graph...</div>
      ) : graph.isError ? (
        <div className="text-xs text-destructive">
          graph failed: {String((graph.error as Error).message)}
        </div>
      ) : (graph.data?.nodes.length ?? 0) === 0 ? (
        <div className="text-xs text-muted-foreground">
          no graph for "{symbol}" (try another symbol or direction)
        </div>
      ) : (
        <div className="flex gap-3">
          <div className="min-w-0 flex-1 rounded-md border border-border">
            <GraphCanvas
              data={graph.data!}
              rootId={selectedNode?.id ?? null}
              onSelect={(n) => update({ sel: n.id })}
              onReRoot={reRoot}
            />
          </div>
          <div className="w-72 shrink-0">
            {selectedNode ? (
              <GraphNodeDetail
                repoPath={repoPath}
                repoId={repoId}
                node={selectedNode}
                onReRoot={reRoot}
              />
            ) : (
              <div className="text-[11px] text-muted-foreground">click a node to inspect it</div>
            )}
          </div>
        </div>
      )}
    </section>
  );
}
