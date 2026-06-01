import { useQuery } from "@tanstack/react-query";
import {
  fetchCallHierarchy,
  fetchDependencyGraph,
  fetchTypeGraph,
  type GraphType,
} from "@/api/graph";

const FETCHERS = {
  call: fetchCallHierarchy,
  type: fetchTypeGraph,
  dependency: fetchDependencyGraph,
};

export function useGraph(
  type: GraphType,
  repoPath: string | null,
  symbolName: string | null,
  file: string | null,
  direction: string,
  depth: number,
) {
  return useQuery({
    queryKey: ["graph", type, repoPath, symbolName, file, direction, depth],
    queryFn: ({ signal }) =>
      FETCHERS[type](repoPath!, symbolName!, file ?? "", direction, depth, signal),
    enabled: Boolean(repoPath) && Boolean(symbolName),
    staleTime: 30_000,
  });
}
