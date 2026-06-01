import { useQuery } from "@tanstack/react-query";
import { getDefinition, findReferences, searchCode } from "@/api/search";

export function useSearch(repoPath: string | null, query: string) {
  return useQuery({
    queryKey: ["search", repoPath, query],
    queryFn: ({ signal: _signal }) => searchCode(repoPath!, query, 25),
    enabled: Boolean(repoPath) && query.trim().length > 0,
    staleTime: 30_000,
  });
}

export function useDefinition(repoPath: string, symbolName: string, file: string, enabled: boolean) {
  return useQuery({
    queryKey: ["definition", repoPath, symbolName, file],
    queryFn: () => getDefinition(repoPath, symbolName, file),
    enabled,
    staleTime: 60_000,
  });
}

export function useReferences(repoPath: string, symbolName: string, file: string, enabled: boolean) {
  return useQuery({
    queryKey: ["references", repoPath, symbolName, file],
    queryFn: () => findReferences(repoPath, symbolName, file),
    enabled,
    staleTime: 60_000,
  });
}
