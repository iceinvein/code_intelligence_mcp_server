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
