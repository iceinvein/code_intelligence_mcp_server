// Shapes mirror src/server/api.rs handlers. Keep field names in sync with Rust.

export type RepoActivity = {
  running: boolean;
  // Job/run detail objects: left as unknown in Phase 1 (only `running` is
  // rendered). These get concrete types when the jobs/activity views land.
  current: unknown | null;
  last_finished: unknown | null;
  latest_index_run: unknown | null;
  latest_search_run: unknown | null;
  last_updated_unix_s: number | null;
};

export type Repo = {
  id: string;
  name: string;
  path: string;
  data_dir: string;
  created_at: string;
  last_accessed: string;
  /** False when the checkout no longer exists on disk. */
  path_exists: boolean;
  /** Base repo id when this index was cloned from another repo, else null. */
  seeded_from: string | null;
  activity: RepoActivity;
};

export type ReposResponse = {
  count: number;
  repos: Repo[];
};

export type StatusResponse = {
  version: string;
  started_at_unix_s: number;
  uptime_s: number;
  registered_repos: number;
  active_sessions: number;
  connected_sessions: number;
  bound_sessions: number;
};

export type VersionResponse = {
  version: string;
  started_at_unix_s: number;
  uptime_s: number;
};

export type JobKind = "manual_reindex" | "initial_bind" | "watch_reindex";
export type JobStatus = "running" | "succeeded" | "failed";

export type Job = {
  id: string;
  kind: JobKind;
  repo_id: string;
  repo_path: string;
  status: JobStatus;
  started_at_unix_s: number;
  finished_at_unix_s: number | null;
  duration_ms: number | null;
  stats: unknown | null;
  error: string | null;
  coalesced_count: number;
};

export type JobsResponse = {
  count: number;
  running: number;
  jobs: Job[];
};

export type Session = {
  session_id: string;
  repo: string | null;
  bound: boolean;
  initialized_at_unix_s: number;
  last_seen_secs_ago: number;
  bind_skipped_reason: string | null;
};

export type SessionsResponse = {
  count: number;
  bound_count: number;
  connected_count: number;
  sessions: Session[];
};

export type RepoStats = {
  symbols: number | null;
  edges: number | null;
  descriptions: number | null;
  undescribed_symbols: number | null;
  last_updated_unix_s: number | null;
  latest_index_run: unknown | null;
  latest_search_run: unknown | null;
  external_indexes: {
    index_count: number;
    symbol_count: number;
    reference_count: number;
    mapped_symbol_count: number;
  } | null;
  external_producers?: Array<{
    id: string;
    language: string;
    tier: string;
    executable: string;
    availability: string;
  }>;
};

export type RepoDetail = {
  id: string;
  name: string;
  path: string;
  data_dir: string;
  created_at: string;
  last_accessed: string;
  stats: RepoStats | null;
};

export type ReindexResponse = {
  status: string;
  job_id: string;
  repo_id: string;
  repo_path: string;
};

export type DeleteResponse = {
  status: string;
  repo_id: string;
  repo_path: string;
  data_dir: string;
};

export type AddRepoResponse = {
  id: string;
  name: string;
  path: string;
  data_dir: string;
  created_at: string;
  last_accessed: string;
};

export type FsEntry = {
  name: string;
  path: string;
  has_git: boolean;
  hidden: boolean;
};

export type FsListing = {
  path: string;
  parent: string | null;
  entries: FsEntry[];
};
