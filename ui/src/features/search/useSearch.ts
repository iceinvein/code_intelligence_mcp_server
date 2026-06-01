import { useQuery } from "@tanstack/react-query";
import { getDefinition, findReferences, searchCode } from "@/api/search";

export function useSearch(repoPath: string | null, query: string) {
  return useQuery({
    queryKey: ["search", repoPath, query],
    queryFn: ({ signal }) => searchCode(repoPath!, query, 25, signal),
    enabled: Boolean(repoPath) && query.trim().length > 0,
    staleTime: 30_000,
  });
}

export function useDefinition(repoPath: string, symbolName: string, file: string, enabled: boolean) {
  return useQuery({
    queryKey: ["definition", repoPath, symbolName, file],
    queryFn: ({ signal }) => getDefinition(repoPath, symbolName, file, signal),
    enabled,
    staleTime: 60_000,
  });
}

export function useReferences(repoPath: string, symbolName: string, file: string, enabled: boolean) {
  return useQuery({
    queryKey: ["references", repoPath, symbolName, file],
    queryFn: ({ signal }) => findReferences(repoPath, symbolName, file, signal),
    enabled,
    staleTime: 60_000,
  });
}
