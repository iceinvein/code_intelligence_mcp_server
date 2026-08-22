import { apiGet } from "@/api/client";
import type { UsageResponse } from "@/api/types";

export function fetchUsage(signal?: AbortSignal): Promise<UsageResponse> {
  return apiGet<UsageResponse>("/usage", signal);
}
