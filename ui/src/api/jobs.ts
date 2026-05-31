import { apiGet } from "@/api/client";
import type { JobsResponse } from "@/api/types";

export function fetchJobs(signal?: AbortSignal): Promise<JobsResponse> {
  return apiGet<JobsResponse>("/jobs", signal);
}
