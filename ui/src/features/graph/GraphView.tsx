import { useEffect } from "react";
import { useSearchParams } from "react-router";
import type { GraphNode, GraphType } from "@/api/graph";
import { GraphCanvas } from "@/features/graph/GraphCanvas";
import { GraphNodeDetail } from "@/features/graph/GraphNodeDetail";
import { SymbolPicker } from "@/features/graph/SymbolPicker";
import { useGraph } from "@/features/graph/useGraph";
import { useRepos } from "@/features/repos/useRepos";
import { cn } from "@/lib/utils";

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
      <div className="mb-4 flex flex-wrap items-center gap-2">
        <span className="text-[0.6875rem] font-medium uppercase tracking-[0.13em] text-label">
          graph
        </span>
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
              className={cn(
                "rounded-md border px-2.5 py-1 text-[0.6875rem] transition-colors duration-150",
                type === t.key
                  ? "border-primary bg-primary/10 text-primary"
                  : "border-input text-muted-foreground hover:bg-muted hover:text-foreground",
              )}
            >
              {t.label}
            </button>
          ))}
        </div>
        <select
          value={direction}
          onChange={(e) => update({ dir: e.target.value })}
          className="h-8 rounded-md border border-input bg-card px-2.5 text-sm outline-none focus-visible:ring-2 focus-visible:ring-ring"
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
          className="h-8 rounded-md border border-input bg-card px-2.5 text-sm outline-none focus-visible:ring-2 focus-visible:ring-ring"
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
          className="ml-auto h-8 rounded-md border border-input bg-card px-2.5 text-sm outline-none focus-visible:ring-2 focus-visible:ring-ring"
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
        <p className="text-sm text-muted-foreground">select a repository.</p>
      ) : !symbol ? (
        <p className="text-sm text-muted-foreground">search for a symbol to root the graph.</p>
      ) : graph.isLoading ? (
        <p className="text-sm text-muted-foreground">building graph…</p>
      ) : graph.isError ? (
        <p className="text-sm text-destructive">
          graph failed: {String((graph.error as Error).message)}
        </p>
      ) : (graph.data?.nodes.length ?? 0) === 0 ? (
        <p className="text-sm text-muted-foreground">
          no graph for <span className="font-mono text-foreground">{symbol}</span> (try another
          symbol or direction).
        </p>
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
              <div className="text-sm text-muted-foreground">click a node to inspect it</div>
            )}
          </div>
        </div>
      )}
    </section>
  );
}
