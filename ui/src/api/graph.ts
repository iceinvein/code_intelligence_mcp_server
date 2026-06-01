import { queryPost } from "@/api/search";

export type GraphNode = {
  id: string;
  name: string;
  kind: string;
  file_path: string;
  exported: boolean;
  line_range: [number, number];
};

export type GraphEdge = {
  from: string;
  to: string;
  edge_type: string;
  at_file: string | null;
  at_line: number | null;
  evidence_count: number;
  resolution: string;
};

export type GraphData = {
  symbol_name: string;
  direction: string;
  depth: number;
  nodes: GraphNode[];
  edges: GraphEdge[];
};

export type GraphType = "call" | "type" | "dependency";

const PATHS: Record<GraphType, string> = {
  call: "/query/call-hierarchy",
  type: "/query/type-graph",
  dependency: "/query/dependency-graph",
};

function fetchGraph(
  type: GraphType,
  repoPath: string,
  symbolName: string,
  file: string,
  direction: string,
  depth: number,
  signal?: AbortSignal,
): Promise<GraphData> {
  return queryPost<GraphData>(
    PATHS[type],
    { repo: repoPath, symbol_name: symbolName, file, direction, depth },
    signal,
  );
}

export function fetchCallHierarchy(
  repoPath: string,
  symbolName: string,
  file: string,
  direction: string,
  depth: number,
  signal?: AbortSignal,
): Promise<GraphData> {
  return fetchGraph("call", repoPath, symbolName, file, direction, depth, signal);
}

export function fetchTypeGraph(
  repoPath: string,
  symbolName: string,
  file: string,
  direction: string,
  depth: number,
  signal?: AbortSignal,
): Promise<GraphData> {
  return fetchGraph("type", repoPath, symbolName, file, direction, depth, signal);
}

export function fetchDependencyGraph(
  repoPath: string,
  symbolName: string,
  file: string,
  direction: string,
  depth: number,
  signal?: AbortSignal,
): Promise<GraphData> {
  return fetchGraph("dependency", repoPath, symbolName, file, direction, depth, signal);
}
