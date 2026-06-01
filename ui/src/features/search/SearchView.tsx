import { useEffect, useState } from "react";
import { useSearchParams } from "react-router";
import { useRepos } from "@/features/repos/useRepos";
import { useSearch } from "@/features/search/useSearch";
import { ResultRow } from "@/features/search/ResultRow";
import { Button } from "@/components/ui/button";

export function SearchView() {
  const [params, setParams] = useSearchParams();
  const reposQuery = useRepos();
  const repos = reposQuery.data?.repos ?? [];

  const repoId = params.get("repo") ?? "";
  const submittedQuery = params.get("q") ?? "";
  const [draft, setDraft] = useState(submittedQuery);

  // Keep the input in sync when the executed query changes from outside (back/forward nav, a shared link).
  // The user typing only changes `draft`, not `submittedQuery`, so this does not clobber in-progress edits.
  useEffect(() => {
    setDraft(submittedQuery);
  }, [submittedQuery]);

  // Auto-select the only repo if none chosen yet. The updater form avoids depending on `params`,
  // so this runs only when the repo list or current selection changes.
  useEffect(() => {
    if (!repoId && repos.length === 1) {
      const onlyRepoId = repos[0]!.id;
      setParams(
        (prev) => {
          const next = new URLSearchParams(prev);
          next.set("repo", onlyRepoId);
          return next;
        },
        { replace: true },
      );
    }
  }, [repoId, repos, setParams]);

  const selectedRepo = repos.find((r) => r.id === repoId) ?? null;
  const repoPath = selectedRepo?.path ?? null;
  const search = useSearch(repoPath, submittedQuery);

  const submit = () => {
    const q = draft.trim();
    const next = new URLSearchParams(params);
    if (q) next.set("q", q);
    else next.delete("q");
    setParams(next);
  };

  const results = search.data?.hits ?? [];

  return (
    <section>
      <h2 className="mb-3 text-[10px] uppercase tracking-[0.18em] text-label">search</h2>
      <div className="mb-4 flex items-center gap-2">
        <input
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") submit();
          }}
          placeholder="search code by meaning..."
          aria-label="search query"
          className="h-7 flex-1 rounded-md border border-border bg-card px-2 font-mono text-[12px] outline-none focus-visible:ring-2 focus-visible:ring-ring"
        />
        <select
          value={repoId}
          aria-label="repository to search"
          onChange={(e) => {
            const next = new URLSearchParams(params);
            next.set("repo", e.target.value);
            setParams(next);
          }}
          className="h-7 rounded-md border border-border bg-card px-2 text-[12px] outline-none"
        >
          <option value="">select repo</option>
          {repos.map((r) => (
            <option key={r.id} value={r.id}>
              {r.name}
            </option>
          ))}
        </select>
        <Button size="sm" disabled={!repoPath || draft.trim() === ""} onClick={submit}>
          search
        </Button>
      </div>

      {!repoPath ? (
        <div className="text-xs text-muted-foreground">select a repository to search</div>
      ) : search.isLoading ? (
        <div className="text-xs text-muted-foreground">searching...</div>
      ) : search.isError ? (
        <div className="text-xs text-destructive">search failed: {String((search.error as Error).message)}</div>
      ) : submittedQuery && results.length === 0 ? (
        <div className="text-xs text-muted-foreground">no results for "{submittedQuery}"</div>
      ) : (
        <>
          {submittedQuery ? (
            <div className="mb-2 text-[10px] uppercase tracking-[0.18em] text-label">
              {results.length} results
            </div>
          ) : null}
          <div>
            {results.map((hit) => (
              <ResultRow key={hit.id} hit={hit} repoPath={repoPath} />
            ))}
          </div>
        </>
      )}
    </section>
  );
}
