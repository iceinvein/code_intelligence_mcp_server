import { apiGet, apiSend } from "@/api/client";

export type PendingConsentRepo = {
  repo_path: string;
  repo_id: string;
  detected: string;
  recommendation: string;
  detail: string | null;
  first_seen_unix_s: number;
  last_seen_unix_s: number;
  occurrences: number;
};

export type DeclinedRepo = {
  repo_path: string;
  repo_id: string;
  detected: string;
};

export type ConsentData = {
  pending: PendingConsentRepo[];
  declined: DeclinedRepo[];
};

export type ConsentDecision = "approve" | "decline";

export type ResolveConsentResponse = {
  ok: boolean;
  status: "ready" | "indexing_started" | "indexing_in_progress" | "declined";
  repo: string;
  repo_id: string;
  job_id?: string;
};

export function fetchConsent(signal?: AbortSignal): Promise<ConsentData> {
  return apiGet<ConsentData>("/consent", signal);
}

export function resolveConsent(
  repo: string,
  decision: ConsentDecision,
): Promise<ResolveConsentResponse> {
  return apiSend<ResolveConsentResponse>("POST", "/consent", { repo, decision });
}
