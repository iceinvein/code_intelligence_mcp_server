import { useState } from "react";
import { useRepos, useRepoDetail, useReindexRepo, useDeleteRepo } from "@/features/repos/useRepos";
import { FolderPickerDialog } from "@/features/repos/FolderPickerDialog";
import { DataSheet, Row, SectionLabel, Field } from "@/components/ui/datasheet";
import { StatusGlyph, type StatusState } from "@/components/ui/status";
import { Skeleton } from "@/components/ui/skeleton";
import { InlineError } from "@/components/ui/inline-error";
import { Button } from "@/components/ui/button";
import { describeError } from "@/lib/errors";
import { formatAgo, formatCount } from "@/lib/format";
import {
  AlertDialog,
  AlertDialogTrigger,
  AlertDialogContent,
  AlertDialogHeader,
  AlertDialogFooter,
  AlertDialogTitle,
  AlertDialogDescription,
  AlertDialogAction,
  AlertDialogCancel,
} from "@/components/ui/alert-dialog";
import type { Repo } from "@/api/types";

export function ReposView() {
  const { data, isLoading, isError, error, refetch } = useRepos();
  const repos = data?.repos ?? [];

  return (
    <section>
      <div className="mb-3 flex items-center justify-between gap-3">
        <SectionLabel className="mb-0" count={isLoading ? undefined : repos.length}>
          repositories
        </SectionLabel>
        <AddRepoForm />
      </div>

      {isLoading ? (
        <ListSkeleton />
      ) : isError ? (
        <InlineError
          message={describeError(error, "couldn't load repositories")}
          onRetry={() => refetch()}
        />
      ) : repos.length === 0 ? (
        <div className="rounded-md border border-dashed border-border px-4 py-8 text-sm text-muted-foreground">
          no repositories registered. use{" "}
          <span className="font-medium text-foreground">add repository</span> to pick a folder and
          start indexing.
        </div>
      ) : (
        <DataSheet>
          {repos.map((repo) => (
            <RepoRow key={repo.id} repo={repo} />
          ))}
        </DataSheet>
      )}
    </section>
  );
}

function AddRepoForm() {
  const [open, setOpen] = useState(false);
  return (
    <>
      <Button size="sm" onClick={() => setOpen(true)}>
        add repository
      </Button>
      <FolderPickerDialog open={open} onOpenChange={setOpen} />
    </>
  );
}

function repoState(repo: Repo): { state: StatusState; label: string } {
  if (repo.activity.running) return { state: "run", label: "indexing" };
  if (repo.activity.last_updated_unix_s != null) return { state: "ok", label: "indexed" };
  return { state: "idle", label: "never indexed" };
}

function RepoRow({ repo }: { repo: Repo }) {
  const [expanded, setExpanded] = useState(false);
  const detail = useRepoDetail(repo.id, expanded);
  const reindex = useReindexRepo();
  const drop = useDeleteRepo();
  const { state, label } = repoState(repo);

  return (
    <div>
      <Row>
        <StatusGlyph state={state} srLabel={label} />
        <button
          type="button"
          className="min-w-0 flex-1 text-left"
          onClick={() => setExpanded((v) => !v)}
          aria-expanded={expanded}
        >
          <div className="truncate text-sm text-foreground">{repo.name}</div>
          <div className="truncate font-mono text-[0.6875rem] text-muted-foreground">{repo.path}</div>
        </button>
        <span className="hidden shrink-0 font-mono text-[0.6875rem] text-muted-foreground sm:inline">
          {repo.activity.last_updated_unix_s != null && !repo.activity.running
            ? formatAgo(repo.activity.last_updated_unix_s)
            : label}
        </span>
        <Button
          variant="outline"
          size="sm"
          disabled={reindex.isPending}
          onClick={() => reindex.mutate(repo.id)}
        >
          {reindex.isPending ? "queued" : "reindex"}
        </Button>
        <AlertDialog>
          <AlertDialogTrigger
            render={
              <Button variant="destructive" size="sm">
                drop
              </Button>
            }
          />
          <AlertDialogContent>
            <AlertDialogHeader>
              <AlertDialogTitle>Drop {repo.name}?</AlertDialogTitle>
              <AlertDialogDescription>
                This removes the registry entry and deletes the on-disk index data directory. This
                cannot be undone.
              </AlertDialogDescription>
            </AlertDialogHeader>
            <AlertDialogFooter>
              <AlertDialogCancel>Cancel</AlertDialogCancel>
              <AlertDialogAction onClick={() => drop.mutate(repo.id)}>Drop repo</AlertDialogAction>
            </AlertDialogFooter>
          </AlertDialogContent>
        </AlertDialog>
      </Row>

      {expanded ? (
        <div className="border-t border-border bg-background/40 px-3.5 py-3">
          {detail.isLoading ? (
            <div className="flex gap-6">
              <Skeleton className="h-7 w-20" />
              <Skeleton className="h-7 w-20" />
              <Skeleton className="h-7 w-20" />
            </div>
          ) : detail.isError ? (
            <span className="text-xs text-destructive">failed to load stats</span>
          ) : detail.data?.stats ? (
            <dl className="flex flex-wrap gap-x-10 gap-y-3">
              <Field label="symbols" value={formatCount(detail.data.stats.symbols)} />
              <Field label="edges" value={formatCount(detail.data.stats.edges)} />
              <Field label="descriptions" value={formatCount(detail.data.stats.descriptions)} />
              <Field label="undescribed" value={formatCount(detail.data.stats.undescribed_symbols)} />
              {detail.data.stats.external_indexes ? (
                <>
                  <Field label="external indexes" value={formatCount(detail.data.stats.external_indexes.index_count)} />
                  <Field label="external refs" value={formatCount(detail.data.stats.external_indexes.reference_count)} />
                  <Field label="mapped external" value={formatCount(detail.data.stats.external_indexes.mapped_symbol_count)} />
                </>
              ) : null}
            </dl>
          ) : (
            <span className="text-xs text-muted-foreground">no stats yet (repo not indexed)</span>
          )}
        </div>
      ) : null}
    </div>
  );
}

function ListSkeleton() {
  return (
    <DataSheet>
      {Array.from({ length: 3 }).map((_, i) => (
        <Row key={i}>
          <Skeleton className="h-2.5 w-2.5 rounded-full" />
          <div className="min-w-0 flex-1">
            <Skeleton className="h-3.5 w-40" />
            <Skeleton className="mt-1.5 h-2.5 w-64" />
          </div>
          <Skeleton className="h-6 w-16" />
        </Row>
      ))}
    </DataSheet>
  );
}
