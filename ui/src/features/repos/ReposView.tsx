import { useRepos } from "@/features/repos/useRepos";
import { Card } from "@/components/ui/card";

export function ReposView() {
  const { data, isLoading, isError, error } = useRepos();

  if (isLoading) return <div className="text-xs text-muted-foreground">loading repositories…</div>;
  if (isError)
    return (
      <div className="text-xs text-destructive">failed to load repositories: {String((error as Error).message)}</div>
    );

  const repos = data?.repos ?? [];
  return (
    <section>
      <h2 className="mb-3 text-[10px] uppercase tracking-[0.18em] text-label">
        repositories · {repos.length}
      </h2>
      {repos.length === 0 ? (
        <div className="text-xs text-muted-foreground">no repositories registered</div>
      ) : (
        <div className="flex flex-col gap-2">
          {repos.map((repo) => (
            <Card key={repo.id} className="flex items-center gap-3 p-3">
              <span
                aria-hidden
                className={repo.activity.running ? "h-2 w-2 rounded-full bg-destructive" : "h-2 w-2 rounded-full bg-primary"}
              />
              <div className="min-w-0 flex-1">
                <div className="truncate text-[13px]">{repo.name}</div>
                <div className="truncate text-[11px] text-muted-foreground">{repo.path}</div>
              </div>
              <span className="text-[11px] text-muted-foreground">
                {repo.activity.running ? "indexing" : "indexed"}
              </span>
            </Card>
          ))}
        </div>
      )}
    </section>
  );
}
