import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { deleteRepo, fetchRepoDetail, fetchRepos, reindexRepo } from "@/api/repos";

export function useRepos() {
  return useQuery({
    queryKey: ["repos"],
    queryFn: ({ signal }) => fetchRepos(signal),
    refetchInterval: 5_000,
  });
}

export function useRepoDetail(id: string, enabled: boolean) {
  return useQuery({
    queryKey: ["repo", id],
    queryFn: ({ signal }) => fetchRepoDetail(id, signal),
    enabled,
  });
}

export function useReindexRepo() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (id: string) => reindexRepo(id),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["repos"] });
      qc.invalidateQueries({ queryKey: ["jobs"] });
    },
  });
}

export function useDeleteRepo() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (id: string) => deleteRepo(id),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["repos"] });
    },
  });
}
