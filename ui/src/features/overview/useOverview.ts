import { useQuery } from "@tanstack/react-query";
import { fetchStatus } from "@/api/repos";

/** Daemon status poll, shared by the header and the overview via one query key. */
export function useStatus() {
  return useQuery({
    queryKey: ["status"],
    queryFn: ({ signal }) => fetchStatus(signal),
    refetchInterval: 5_000,
  });
}
