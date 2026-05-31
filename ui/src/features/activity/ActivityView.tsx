import { useJobs, useSessions } from "@/features/activity/useActivity";
import { Card } from "@/components/ui/card";
import type { Job, Session } from "@/api/types";

export function ActivityView() {
  return (
    <div className="flex flex-col gap-6">
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
      <h2 className="mb-3 text-[10px] uppercase tracking-[0.18em] text-label">
        jobs &middot; {data?.running ?? 0} running
      </h2>
      {isLoading ? (
        <div className="text-xs text-muted-foreground">loading jobs...</div>
      ) : isError ? (
        <div className="text-xs text-destructive">failed to load jobs</div>
      ) : jobs.length === 0 ? (
        <div className="text-xs text-muted-foreground">no recent jobs</div>
      ) : (
        <div className="flex flex-col gap-2">
          {jobs.map((job) => (
            <JobRow key={job.id} job={job} />
          ))}
        </div>
      )}
    </section>
  );
}

function JobRow({ job }: { job: Job }) {
  const color =
    job.status === "running" ? "text-primary" : job.status === "failed" ? "text-destructive" : "text-muted-foreground";
  return (
    <Card className="flex items-center gap-3 p-3 font-mono text-[11px]">
      <span className={color}>{job.status}</span>
      <span className="text-muted-foreground">{job.kind}</span>
      <span className="min-w-0 flex-1 truncate text-foreground">{job.repo_path}</span>
      {job.duration_ms !== null ? <span className="text-muted-foreground">{job.duration_ms}ms</span> : null}
      {job.coalesced_count > 0 ? <span className="text-muted-foreground">x{job.coalesced_count}</span> : null}
    </Card>
  );
}

function SessionsSection() {
  const { data, isLoading, isError } = useSessions();
  const sessions = data?.sessions ?? [];
  return (
    <section>
      <h2 className="mb-3 text-[10px] uppercase tracking-[0.18em] text-label">
        sessions &middot; {data?.bound_count ?? 0} bound / {data?.connected_count ?? 0} connected
      </h2>
      {isLoading ? (
        <div className="text-xs text-muted-foreground">loading sessions...</div>
      ) : isError ? (
        <div className="text-xs text-destructive">failed to load sessions</div>
      ) : sessions.length === 0 ? (
        <div className="text-xs text-muted-foreground">no connected sessions</div>
      ) : (
        <div className="flex flex-col gap-2">
          {sessions.map((s) => (
            <SessionRow key={s.session_id} session={s} />
          ))}
        </div>
      )}
    </section>
  );
}

function SessionRow({ session }: { session: Session }) {
  return (
    <Card className="flex items-center gap-3 p-3 font-mono text-[11px]">
      <span className={session.bound ? "text-primary" : "text-muted-foreground"}>
        {session.bound ? "bound" : "unbound"}
      </span>
      <span className="min-w-0 flex-1 truncate text-foreground">{session.repo ?? session.bind_skipped_reason ?? "(no repo)"}</span>
      <span className="text-muted-foreground">{session.last_seen_secs_ago}s ago</span>
    </Card>
  );
}
