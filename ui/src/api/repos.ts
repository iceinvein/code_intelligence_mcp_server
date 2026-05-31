import { apiGet, apiSend } from "@/api/client";
import type {
  ReposResponse,
  StatusResponse,
  VersionResponse,
  RepoDetail,
  ReindexResponse,
  DeleteResponse,
} from "@/api/types";

export function fetchRepos(signal?: AbortSignal): Promise<ReposResponse> {
  return apiGet<ReposResponse>("/repos", signal);
}

export function fetchStatus(signal?: AbortSignal): Promise<StatusResponse> {
  return apiGet<StatusResponse>("/status", signal);
}

export function fetchVersion(signal?: AbortSignal): Promise<VersionResponse> {
  return apiGet<VersionResponse>("/version", signal);
}

export function fetchRepoDetail(id: string, signal?: AbortSignal): Promise<RepoDetail> {
  return apiGet<RepoDetail>(`/repos/${encodeURIComponent(id)}`, signal);
}

export function reindexRepo(id: string): Promise<ReindexResponse> {
  return apiSend<ReindexResponse>("POST", `/repos/${encodeURIComponent(id)}/reindex`);
}

export function deleteRepo(id: string): Promise<DeleteResponse> {
  return apiSend<DeleteResponse>("DELETE", `/repos/${encodeURIComponent(id)}`);
}
