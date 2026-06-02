import { useJobs, useSessions } from "@/features/activity/useActivity";
import { JOB_KIND_LABEL, JOB_STATE } from "@/features/activity/labels";
import { DataSheet, Row, SectionLabel } from "@/components/ui/datasheet";
import { StatusGlyph } from "@/components/ui/status";
import { Skeleton } from "@/components/ui/skeleton";
import { formatAgo, formatDuration } from "@/lib/format";
import type { Job, Session } from "@/api/types";

export function ActivityView() {
  return (
    <div className="flex flex-col gap-8">
      <JobsSection />
      <SessionsSection />
    </div>
  );
}

function JobsSection() {
  const { data, isLoading, isError } = useJobs();
  const jobs = data?.jobs ?? [];
  return (
    <section>
      <SectionLabel count={data ? `${data.running} running` : undefined}>jobs</SectionLabel>
      {isLoading ? (
        <ListSkeleton />
      ) : isError ? (
        <p className="text-sm text-destructive">failed to load jobs</p>
      ) : jobs.length === 0 ? (
        <Empty>no jobs have run yet. indexing activity will appear here.</Empty>
      ) : (
        <DataSheet>
          {jobs.map((job) => (
            <JobRow key={job.id} job={job} />
          ))}
        </DataSheet>
      )}
    </section>
  );
}

function JobRow({ job }: { job: Job }) {
  return (
    <Row>
      <StatusGlyph state={JOB_STATE[job.status]} label={job.status} className="w-24 shrink-0 text-xs" />
      <span className="w-28 shrink-0 truncate text-sm text-foreground">{JOB_KIND_LABEL[job.kind]}</span>
      <span className="min-w-0 flex-1 truncate font-mono text-[0.6875rem] text-muted-foreground">
        {job.repo_path}
      </span>
      {job.coalesced_count > 0 ? (
        <span className="shrink-0 font-mono text-[0.6875rem] text-muted-foreground" title="coalesced runs">
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

function SessionsSection() {
  const { data, isLoading, isError } = useSessions();
  const sessions = data?.sessions ?? [];
  return (
    <section>
      <SectionLabel
        count={data ? `${data.bound_count} bound / ${data.connected_count} connected` : undefined}
      >
        sessions
      </SectionLabel>
      {isLoading ? (
        <ListSkeleton />
      ) : isError ? (
        <p className="text-sm text-destructive">failed to load sessions</p>
      ) : sessions.length === 0 ? (
        <Empty>no MCP sessions connected. clients appear here once they bind a repo.</Empty>
      ) : (
        <DataSheet>
          {sessions.map((s) => (
            <SessionRow key={s.session_id} session={s} />
          ))}
        </DataSheet>
      )}
    </section>
  );
}

function SessionRow({ session }: { session: Session }) {
  return (
    <Row>
      <StatusGlyph
        state={session.bound ? "ok" : "idle"}
        label={session.bound ? "bound" : "unbound"}
        className="w-20 shrink-0 text-xs"
      />
      <span className="min-w-0 flex-1 truncate font-mono text-[0.6875rem] text-muted-foreground">
        {session.repo ?? session.bind_skipped_reason ?? "(no repo)"}
      </span>
      <span className="shrink-0 font-mono text-[0.6875rem] tabular-nums text-muted-foreground">
        {session.last_seen_secs_ago}s ago
      </span>
    </Row>
  );
}

function Empty({ children }: { children: React.ReactNode }) {
  return (
    <div className="rounded-md border border-dashed border-border px-4 py-6 text-sm text-muted-foreground">
      {children}
    </div>
  );
}

function ListSkeleton() {
  return (
    <DataSheet>
      {Array.from({ length: 3 }).map((_, i) => (
        <Row key={i}>
          <Skeleton className="h-2.5 w-2.5 rounded-full" />
          <Skeleton className="h-3.5 w-24" />
          <div className="flex-1" />
          <Skeleton className="h-3 w-14" />
        </Row>
      ))}
    </DataSheet>
  );
}
