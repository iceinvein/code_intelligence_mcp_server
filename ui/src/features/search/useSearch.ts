import { useQuery } from "@tanstack/react-query";
import { searchCode } from "@/api/search";

export function useSearch(repoPath: string | null, query: string) {
  return useQuery({
    queryKey: ["search", repoPath, query],
    queryFn: ({ signal: _signal }) => searchCode(repoPath!, query, 25),
    enabled: Boolean(repoPath) && query.trim().length > 0,
    staleTime: 30_000,
  });
}
