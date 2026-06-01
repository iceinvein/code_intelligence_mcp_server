import { useQuery } from "@tanstack/react-query";
import { fetchFiles, fetchFileSymbols, fetchUsageExamples } from "@/api/symbols";

export function useFiles(repoPath: string | null) {
  return useQuery({
    queryKey: ["files", repoPath],
    queryFn: ({ signal }) => fetchFiles(repoPath!, signal),
    enabled: Boolean(repoPath),
    staleTime: 30_000,
  });
}

export function useFileSymbols(repoPath: string | null, filePath: string | null) {
  return useQuery({
    queryKey: ["file-symbols", repoPath, filePath],
    queryFn: ({ signal }) => fetchFileSymbols(repoPath!, filePath!, signal),
    enabled: Boolean(repoPath) && Boolean(filePath),
    staleTime: 30_000,
  });
}

export function useUsageExamples(
  repoPath: string | null,
  symbolName: string | null,
  file: string | null,
  enabled: boolean,
) {
  return useQuery({
    queryKey: ["usage-examples", repoPath, symbolName, file],
    queryFn: ({ signal }) => fetchUsageExamples(repoPath!, symbolName!, file ?? undefined, signal),
    enabled: enabled && Boolean(repoPath) && Boolean(symbolName),
    staleTime: 60_000,
  });
}
