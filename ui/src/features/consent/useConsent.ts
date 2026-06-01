import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { fetchConsent, resolveConsent, type ConsentDecision } from "@/api/consent";

export function useConsent() {
  return useQuery({
    queryKey: ["consent"],
    queryFn: ({ signal }) => fetchConsent(signal),
    refetchInterval: 5_000,
  });
}

export function useResolveConsent() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({ repo, decision }: { repo: string; decision: ConsentDecision }) =>
      resolveConsent(repo, decision),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["consent"] });
      qc.invalidateQueries({ queryKey: ["repos"] });
      qc.invalidateQueries({ queryKey: ["jobs"] });
    },
  });
}
