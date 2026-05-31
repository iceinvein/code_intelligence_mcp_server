import { useQuery } from "@tanstack/react-query";
import { fetchJobs } from "@/api/jobs";
import { fetchSessions } from "@/api/sessions";

export function useJobs() {
  return useQuery({
    queryKey: ["jobs"],
    queryFn: ({ signal }) => fetchJobs(signal),
    refetchInterval: 3_000,
  });
}

export function useSessions() {
  return useQuery({
    queryKey: ["sessions"],
    queryFn: ({ signal }) => fetchSessions(signal),
    refetchInterval: 5_000,
  });
}
