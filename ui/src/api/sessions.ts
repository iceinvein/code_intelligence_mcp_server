import { apiGet } from "@/api/client";
import type { SessionsResponse } from "@/api/types";

export function fetchSessions(signal?: AbortSignal): Promise<SessionsResponse> {
  return apiGet<SessionsResponse>("/sessions", signal);
}
