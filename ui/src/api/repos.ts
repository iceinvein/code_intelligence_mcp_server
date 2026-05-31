import { apiGet } from "@/api/client";
import type { ReposResponse, StatusResponse, VersionResponse } from "@/api/types";

export function fetchRepos(signal?: AbortSignal): Promise<ReposResponse> {
  return apiGet<ReposResponse>("/repos", signal);
}

export function fetchStatus(signal?: AbortSignal): Promise<StatusResponse> {
  return apiGet<StatusResponse>("/status", signal);
}

export function fetchVersion(signal?: AbortSignal): Promise<VersionResponse> {
  return apiGet<VersionResponse>("/version", signal);
}
