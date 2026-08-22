import { useQuery } from "@tanstack/react-query";
import { fetchUsage } from "@/api/usage";

export function useUsage() {
  return useQuery({
    queryKey: ["usage"],
    queryFn: ({ signal }) => fetchUsage(signal),
    refetchInterval: 10_000,
  });
}
