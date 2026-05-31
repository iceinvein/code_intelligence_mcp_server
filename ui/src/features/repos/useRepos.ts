import { useQuery } from "@tanstack/react-query";
import { fetchRepos } from "@/api/repos";

export function useRepos() {
  return useQuery({
    queryKey: ["repos"],
    queryFn: ({ signal }) => fetchRepos(signal),
    refetchInterval: 5_000,
  });
}
