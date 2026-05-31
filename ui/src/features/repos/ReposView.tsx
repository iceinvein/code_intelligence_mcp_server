import { useState } from "react";
import { useRepos, useRepoDetail, useReindexRepo, useDeleteRepo, useAddRepo } from "@/features/repos/useRepos";
import { Card } from "@/components/ui/card";
import { Button } from "@/components/ui/button";
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
  const { data, isLoading, isError, error } = useRepos();

  if (isLoading) return <div className="text-xs text-muted-foreground">loading repositories...</div>;
  if (isError)
    return (
      <div className="text-xs text-destructive">failed to load repositories: {String((error as Error).message)}</div>
    );

  const repos = data?.repos ?? [];
  return (
    <section>
      <h2 className="mb-3 text-[10px] uppercase tracking-[0.18em] text-label">
        repositories &middot; {repos.length}
      </h2>
      <AddRepoForm />
      {repos.length === 0 ? (
        <div className="text-xs text-muted-foreground">no repositories registered</div>
      ) : (
        <div className="flex flex-col gap-2">
          {repos.map((repo) => (
            <RepoRow key={repo.id} repo={repo} />
          ))}
        </div>
      )}
    </section>
  );
}

function AddRepoForm() {
  const [path, setPath] = useState("");
  const add = useAddRepo();
  const submit = () => {
    const trimmed = path.trim();
    if (!trimmed) return;
    add.mutate(trimmed, { onSuccess: () => setPath("") });
  };
  return (
    <div className="mb-4 flex items-center gap-2">
      <input
        value={path}
        onChange={(e) => setPath(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter") submit();
        }}
        placeholder="/absolute/path/to/repo"
        aria-label="repository path to add"
        className="h-7 flex-1 rounded-md border border-border bg-card px-2 font-mono text-[12px] outline-none focus-visible:ring-2 focus-visible:ring-ring"
      />
      <Button size="sm" disabled={add.isPending || path.trim() === ""} onClick={submit}>
        {add.isPending ? "adding" : "add repo"}
      </Button>
      {add.isError ? (
        <span className="text-[11px] text-destructive">{String((add.error as Error).message)}</span>
      ) : null}
    </div>
  );
}

function RepoRow({ repo }: { repo: Repo }) {
  const [expanded, setExpanded] = useState(false);
  const detail = useRepoDetail(repo.id, expanded);
  const reindex = useReindexRepo();
  const drop = useDeleteRepo();
  const running = repo.activity.running;

  return (
    <Card className="p-3">
      <div className="flex items-center gap-3">
        <span
          aria-hidden
          className={running ? "h-2 w-2 rounded-full bg-primary animate-pulse" : "h-2 w-2 rounded-full bg-primary"}
        />
        <button
          className="min-w-0 flex-1 text-left"
          onClick={() => setExpanded((v) => !v)}
          aria-expanded={expanded}
        >
          <div className="truncate text-[13px]">{repo.name}</div>
          <div className="truncate text-[11px] text-muted-foreground">{repo.path}</div>
        </button>
        <span className="text-[11px] text-muted-foreground">{running ? "indexing" : "indexed"}</span>
        <Button
          variant="outline"
          size="sm"
          disabled={reindex.isPending}
          onClick={() => reindex.mutate(repo.id)}
        >
          {reindex.isPending ? "queued" : "reindex"}
        </Button>
        <AlertDialog>
          <AlertDialogTrigger asChild>
            <Button variant="destructive" size="sm">
              drop
            </Button>
          </AlertDialogTrigger>
          <AlertDialogContent>
            <AlertDialogHeader>
              <AlertDialogTitle>Drop {repo.name}?</AlertDialogTitle>
              <AlertDialogDescription>
                This removes the registry entry and deletes the on-disk index data directory. This cannot be undone.
              </AlertDialogDescription>
            </AlertDialogHeader>
            <AlertDialogFooter>
              <AlertDialogCancel>Cancel</AlertDialogCancel>
              <AlertDialogAction onClick={() => drop.mutate(repo.id)}>Drop repo</AlertDialogAction>
            </AlertDialogFooter>
          </AlertDialogContent>
        </AlertDialog>
      </div>
      {expanded ? (
        <div className="mt-3 border-t border-border pt-3 text-[11px] text-muted-foreground">
          {detail.isLoading ? (
            <span>loading stats...</span>
          ) : detail.isError ? (
            <span className="text-destructive">failed to load stats</span>
          ) : detail.data?.stats ? (
            <div className="flex flex-wrap gap-x-6 gap-y-1 font-mono">
              <span>symbols: {detail.data.stats.symbols ?? "n/a"}</span>
              <span>edges: {detail.data.stats.edges ?? "n/a"}</span>
              <span>descriptions: {detail.data.stats.descriptions ?? "n/a"}</span>
              <span>undescribed: {detail.data.stats.undescribed_symbols ?? "n/a"}</span>
            </div>
          ) : (
            <span>no stats yet (repo not indexed)</span>
          )}
        </div>
      ) : null}
    </Card>
  );
}
