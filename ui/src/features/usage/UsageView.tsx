import { useUsage } from "@/features/usage/useUsage";
import { DataSheet, Row, SectionLabel } from "@/components/ui/datasheet";
import { StatusGlyph } from "@/components/ui/status";
import { Skeleton } from "@/components/ui/skeleton";
import { formatAgo, formatCount, formatDuration } from "@/lib/format";
import type { DailyUsage, RepoUsage, RecentSearchRun } from "@/api/types";

type UsageQuery = ReturnType<typeof useUsage>;

export function UsageView() {
  const usage = useUsage();
  return (
    <div className="flex flex-col gap-8">
      <TotalsSection usage={usage} />
      <ReposSection usage={usage} />
      <RecentSearchesSection usage={usage} />
    </div>
  );
}

function TotalsSection({ usage }: { usage: UsageQuery }) {
  const { data, isLoading, isError } = usage;
  const searches = data?.totals.searches ?? 0;
  const cacheHits = data?.totals.cache_hits ?? 0;
  const hitRate = searches > 0 ? Math.round((cacheHits / searches) * 100) : null;
  const indexRuns = (data?.repos ?? []).reduce((acc, r) => acc + r.index_run_count, 0);
  return (
    <section>
      <SectionLabel>usage totals</SectionLabel>
      {isLoading ? (
        <ListSkeleton />
      ) : isError ? (
        <p className="text-sm text-destructive">failed to load usage</p>
      ) : (
        <DataSheet>
          <Row>
            <span className="w-32 shrink-0 text-sm text-muted-foreground">searches</span>
            <span className="flex-1 font-mono text-sm tabular-nums text-foreground">
              {formatCount(searches)}
            </span>
          </Row>
          <Row>
            <span className="w-32 shrink-0 text-sm text-muted-foreground">cache hit rate</span>
            <span className="flex-1 font-mono text-sm tabular-nums text-foreground">
              {hitRate == null ? "–" : `${hitRate}%`}
            </span>
          </Row>
          <Row>
            <span className="w-32 shrink-0 text-sm text-muted-foreground">index runs</span>
            <span className="flex-1 font-mono text-sm tabular-nums text-foreground">
              {formatCount(indexRuns)}
            </span>
          </Row>
        </DataSheet>
      )}
    </section>
  );
}

function ReposSection({ usage }: { usage: UsageQuery }) {
  const { data, isLoading, isError } = usage;
  const repos = data?.repos ?? [];
  const windowDays = data?.window_days ?? 14;
  return (
    <section>
      <SectionLabel count={data ? `${repos.length} indexed` : undefined}>
        repositories
      </SectionLabel>
      {isLoading ? (
        <ListSkeleton />
      ) : isError ? (
        <p className="text-sm text-destructive">failed to load usage</p>
      ) : repos.length === 0 ? (
        <Empty>no repositories registered. add one to see its usage here.</Empty>
      ) : (
        <DataSheet>
          {repos.map((repo) => (
            <RepoRow key={repo.id} repo={repo} windowDays={windowDays} />
          ))}
        </DataSheet>
      )}
    </section>
  );
}

function RepoRow({ repo, windowDays }: { repo: RepoUsage; windowDays: number }) {
  return (
    <Row>
      <DailyBars daily={repo.daily} days={windowDays} />
      <span className="min-w-0 flex-1 truncate text-sm text-foreground" title={repo.path}>
        {repo.name}
      </span>
      <span
        className="shrink-0 font-mono text-[0.6875rem] tabular-nums text-muted-foreground"
        title="searches"
      >
        {formatCount(repo.search_total)}
      </span>
      <span
        className="w-14 shrink-0 text-right font-mono text-[0.6875rem] tabular-nums text-muted-foreground"
        title="avg search duration"
      >
        {formatDuration(repo.avg_duration_ms)}
      </span>
      <span
        className="w-20 shrink-0 text-right font-mono text-[0.6875rem] tabular-nums text-muted-foreground"
        title="last search"
      >
        {repo.last_search_at_unix_s == null
          ? "never"
          : formatAgo(repo.last_search_at_unix_s)}
      </span>
    </Row>
  );
}

/// Zero-filled UTC-day bars over the trailing `days` window; tallest day is full height.
function DailyBars({
  daily,
  days,
  nowUnixS,
}: {
  daily: DailyUsage[];
  days: number;
  nowUnixS?: number;
}) {
  const now = nowUnixS ?? Math.floor(Date.now() / 1000);
  const byDay = new Map(daily.map((d) => [d.day, d.searches]));
  const buckets: { day: string; searches: number }[] = [];
  for (let i = days - 1; i >= 0; i--) {
    const ts = now - i * 86_400;
    const day = new Date(ts * 1000).toISOString().slice(0, 10);
    buckets.push({ day, searches: byDay.get(day) ?? 0 });
  }
  const max = Math.max(1, ...buckets.map((b) => b.searches));
  return (
    <span
      className="flex h-6 w-28 shrink-0 items-end gap-px"
      role="img"
      aria-label={`searches per day over ${days} days`}
    >
      {buckets.map((b) => (
        <span
          key={b.day}
          title={`${b.day}: ${b.searches} searches`}
          className={`w-full rounded-sm ${b.searches > 0 ? "bg-primary/70" : "bg-muted"}`}
          style={{ height: `${Math.max(8, (b.searches / max) * 100)}%` }}
        />
      ))}
    </span>
  );
}

function RecentSearchesSection({ usage }: { usage: UsageQuery }) {
  const { data, isLoading, isError } = usage;
  const runs = data?.recent_runs ?? [];
  return (
    <section>
      <SectionLabel count={data ? `latest ${runs.length}` : undefined}>
        recent searches
      </SectionLabel>
      {isLoading ? (
        <ListSkeleton />
      ) : isError ? (
        <p className="text-sm text-destructive">failed to load usage</p>
      ) : runs.length === 0 ? (
        <Empty>no searches recorded yet. agent queries appear here as they run.</Empty>
      ) : (
        <DataSheet>
          {runs.map((run, i) => (
            <RunRow key={`${run.repo_id}-${run.started_at_unix_s}-${i}`} run={run} />
          ))}
        </DataSheet>
      )}
    </section>
  );
}

function RunRow({ run }: { run: RecentSearchRun }) {
  const hit = run.cache_status === "hit";
  return (
    <Row>
      <StatusGlyph
        state={hit ? "ok" : "idle"}
        label={run.cache_status}
        className="w-16 shrink-0 text-xs"
      />
      <span className="w-36 shrink-0 truncate font-mono text-[0.6875rem] text-muted-foreground">
        {run.repo_name}
      </span>
      <span className="min-w-0 flex-1 truncate text-sm text-foreground">
        {run.query_text ?? <span className="text-muted-foreground">(query not stored)</span>}
      </span>
      <span
        className="shrink-0 font-mono text-[0.6875rem] tabular-nums text-muted-foreground"
        title="results"
      >
        {formatCount(run.result_count)}
      </span>
      <span className="w-14 shrink-0 text-right font-mono text-[0.6875rem] tabular-nums text-muted-foreground">
        {formatDuration(run.duration_ms)}
      </span>
      <span className="w-20 shrink-0 text-right font-mono text-[0.6875rem] tabular-nums text-muted-foreground">
        {formatAgo(run.started_at_unix_s)}
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
