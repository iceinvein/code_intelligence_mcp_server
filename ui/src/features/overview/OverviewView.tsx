import { type ReactNode } from "react";
import { Link } from "react-router";
import { useStatus } from "@/features/overview/useOverview";
import { useRepos } from "@/features/repos/useRepos";
import { useJobs, useSessions } from "@/features/activity/useActivity";
import { JOB_KIND_LABEL, JOB_STATE } from "@/features/activity/labels";
import { DataSheet, Row, SectionLabel } from "@/components/ui/datasheet";
import { StatusGlyph, type StatusState } from "@/components/ui/status";
import { Skeleton } from "@/components/ui/skeleton";
import { InlineError } from "@/components/ui/inline-error";
import { formatAgo, formatCount, formatDuration, formatUptime } from "@/lib/format";
import type { Job, Repo } from "@/api/types";

const REPO_PREVIEW = 6;
const JOB_PREVIEW = 6;

export function OverviewView() {
  const status = useStatus();
  const repos = useRepos();
  const jobs = useJobs();
  const sessions = useSessions();

  const unreachable = status.isError;
  const s = status.data;
  const repoList = repos.data?.repos ?? [];
  const indexing = repoList.filter((r) => r.activity.running).length;
  const runningJobs = jobs.data?.running ?? 0;

  return (
    <div className="flex flex-col gap-8">
      {/* Daemon line – the headline. Sparse and calm. */}
      <section aria-labelledby="daemon-line">
        <h1 id="daemon-line" className="sr-only">
          daemon status
        </h1>
        {unreachable ? (
          <div className="flex items-start gap-2.5 rounded-md border border-destructive/40 bg-destructive/5 px-4 py-3 text-sm">
            <StatusGlyph state="fail" srLabel="daemon unreachable" className="mt-0.5" />
            <span className="text-foreground">
              <span className="font-medium">daemon unreachable</span> at{" "}
              <span className="font-mono text-[0.8125rem]">127.0.0.1:17802</span>. check that it is
              running, then reload.
            </span>
          </div>
        ) : (
          <div className="flex flex-wrap items-baseline gap-x-4 gap-y-1">
            <StatusGlyph
              state="ok"
              label="daemon running"
              className="text-base [&>span]:font-medium [&>span]:text-foreground"
            />
            <span className="font-mono text-xs text-muted-foreground">127.0.0.1:17802</span>
            {s ? (
              <span className="font-mono text-xs text-muted-foreground">
                v{s.version} · up {formatUptime(s.uptime_s)}
              </span>
            ) : null}
          </div>
        )}
      </section>

      {/* Vital readout – one hairline-divided bar, not four hero cards. */}
      <section aria-label="daemon vitals">
        <dl className="grid grid-cols-2 gap-px overflow-hidden rounded-md border border-border bg-border sm:grid-cols-4">
          <Vital
            label="daemon"
            loading={status.isLoading && !s}
            value={
              <span className="flex items-center gap-2 text-base">
                <StatusGlyph state={unreachable ? "fail" : "ok"} srLabel="daemon" />
                <span>{unreachable ? "down" : "running"}</span>
              </span>
            }
            sub={s ? `up ${formatUptime(s.uptime_s)}` : unreachable ? "no response" : "–"}
          />
          <Vital
            label="repositories"
            loading={repos.isLoading}
            value={formatCount(repoList.length)}
            sub={indexing > 0 ? `${indexing} indexing` : "all idle"}
            subState={indexing > 0 ? "run" : undefined}
          />
          <Vital
            label="sessions"
            loading={sessions.isLoading}
            value={formatCount(sessions.data?.bound_count ?? 0)}
            sub={`${sessions.data?.connected_count ?? 0} connected`}
          />
          <Vital
            label="jobs"
            loading={jobs.isLoading}
            value={formatCount(runningJobs)}
            sub={runningJobs > 0 ? "running" : "idle"}
            subState={runningJobs > 0 ? "run" : undefined}
          />
        </dl>
      </section>

      {/* Repositories – condensed, links out to full management. */}
      <section>
        <div className="mb-3 flex items-baseline justify-between">
          <SectionLabel className="mb-0" count={repos.isLoading ? undefined : repoList.length}>
            repositories
          </SectionLabel>
          <Link
            to="/repos"
            className="text-xs text-muted-foreground underline-offset-4 hover:text-primary hover:underline"
          >
            manage
          </Link>
        </div>
        {repos.isLoading ? (
          <RowsSkeleton rows={3} />
        ) : repos.isError ? (
          <InlineError message="failed to load repositories" onRetry={() => repos.refetch()} />
        ) : repoList.length === 0 ? (
          <EmptyState>
            no repositories indexed yet. add one in{" "}
            <Link to="/repos" className="text-primary underline-offset-4 hover:underline">
              repositories
            </Link>{" "}
            to start indexing.
          </EmptyState>
        ) : (
          <>
            <DataSheet>
              {repoList.slice(0, REPO_PREVIEW).map((repo) => (
                <RepoLine key={repo.id} repo={repo} />
              ))}
            </DataSheet>
            {repoList.length > REPO_PREVIEW ? (
              <Link
                to="/repos"
                className="mt-2 inline-block text-xs text-muted-foreground underline-offset-4 hover:text-primary hover:underline"
              >
                view all {repoList.length}
              </Link>
            ) : null}
          </>
        )}
      </section>

      {/* Recent jobs – the activity tail. */}
      <section>
        <div className="mb-3 flex items-baseline justify-between">
          <SectionLabel className="mb-0" count={runningJobs > 0 ? `${runningJobs} running` : undefined}>
            recent jobs
          </SectionLabel>
          <Link
            to="/activity"
            className="text-xs text-muted-foreground underline-offset-4 hover:text-primary hover:underline"
          >
            all activity
          </Link>
        </div>
        {jobs.isLoading ? (
          <RowsSkeleton rows={3} />
        ) : jobs.isError ? (
          <InlineError message="failed to load jobs" onRetry={() => jobs.refetch()} />
        ) : (jobs.data?.jobs.length ?? 0) === 0 ? (
          <EmptyState>no jobs have run yet. indexing activity will appear here.</EmptyState>
        ) : (
          <DataSheet>
            {jobs.data!.jobs.slice(0, JOB_PREVIEW).map((job) => (
              <JobLine key={job.id} job={job} />
            ))}
          </DataSheet>
        )}
      </section>
    </div>
  );
}

function Vital({
  label,
  value,
  sub,
  subState,
  loading,
}: {
  label: string;
  value: ReactNode;
  sub: string;
  subState?: StatusState;
  loading?: boolean;
}) {
  return (
    <div className="bg-card px-4 py-3.5">
      <dt className="text-[0.625rem] font-medium uppercase tracking-[0.12em] text-label">{label}</dt>
      {loading ? (
        <Skeleton className="mt-2 h-5 w-16" />
      ) : (
        <dd className="mt-1.5 font-mono text-[1.375rem] leading-none tabular-nums text-foreground">
          {value}
        </dd>
      )}
      <div className="mt-2 flex items-center gap-1.5 text-[0.6875rem] text-muted-foreground">
        {subState ? <StatusGlyph state={subState} srLabel={sub} /> : null}
        <span>{sub}</span>
      </div>
    </div>
  );
}

function repoState(repo: Repo): { state: StatusState; label: string } {
  if (repo.activity.running) return { state: "run", label: "indexing" };
  if (repo.activity.last_updated_unix_s != null) return { state: "ok", label: "indexed" };
  return { state: "idle", label: "never indexed" };
}

function RepoLine({ repo }: { repo: Repo }) {
  const { state, label } = repoState(repo);
  return (
    <Row>
      <StatusGlyph state={state} srLabel={label} />
      <Link
        to={`/search?repo=${encodeURIComponent(repo.id)}`}
        className="min-w-0 flex-1 truncate text-sm text-foreground underline-offset-4 hover:text-primary hover:underline"
      >
        {repo.name}
      </Link>
      <span className="hidden min-w-0 flex-1 truncate font-mono text-[0.6875rem] text-muted-foreground sm:block">
        {repo.path}
      </span>
      <span className="shrink-0 font-mono text-[0.6875rem] text-muted-foreground">
        {repo.activity.last_updated_unix_s != null
          ? formatAgo(repo.activity.last_updated_unix_s)
          : label}
      </span>
    </Row>
  );
}

function JobLine({ job }: { job: Job }) {
  return (
    <Row>
      <StatusGlyph state={JOB_STATE[job.status]} srLabel={job.status} />
      <span className="w-28 shrink-0 truncate text-sm text-foreground">
        {JOB_KIND_LABEL[job.kind]}
      </span>
      <span className="min-w-0 flex-1 truncate font-mono text-[0.6875rem] text-muted-foreground">
        {job.repo_path}
      </span>
      {job.coalesced_count > 0 ? (
        <span className="shrink-0 font-mono text-[0.6875rem] text-muted-foreground">
          ×{job.coalesced_count + 1}
        </span>
      ) : null}
      <span className="shrink-0 font-mono text-[0.6875rem] tabular-nums text-muted-foreground">
        {job.status === "running"
          ? formatAgo(job.started_at_unix_s)
          : job.duration_ms != null
            ? formatDuration(job.duration_ms)
            : formatAgo(job.finished_at_unix_s)}
      </span>
    </Row>
  );
}

function EmptyState({ children }: { children: ReactNode }) {
  return (
    <div className="rounded-md border border-dashed border-border px-4 py-6 text-sm text-muted-foreground">
      {children}
    </div>
  );
}

function RowsSkeleton({ rows }: { rows: number }) {
  return (
    <DataSheet>
      {Array.from({ length: rows }).map((_, i) => (
        <Row key={i}>
          <Skeleton className="h-2.5 w-2.5 rounded-full" />
          <Skeleton className="h-3.5 w-40" />
          <div className="flex-1" />
          <Skeleton className="h-3 w-16" />
        </Row>
      ))}
    </DataSheet>
  );
}
