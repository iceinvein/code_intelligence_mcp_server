import type { Job } from "@/api/types";
import type { StatusState } from "@/components/ui/status";

/** Job lifecycle status mapped to the shared signal vocabulary. */
export const JOB_STATE: Record<Job["status"], StatusState> = {
  running: "run",
  succeeded: "ok",
  failed: "fail",
};

/** Job kinds, humanized for display (raw enum values stay in the API/logs). */
export const JOB_KIND_LABEL: Record<Job["kind"], string> = {
  manual_reindex: "manual reindex",
  initial_bind: "initial bind",
  watch_reindex: "watch reindex",
};
