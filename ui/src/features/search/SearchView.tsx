import { useEffect, useState } from "react";
import { useSearchParams } from "react-router";
import { useRepos } from "@/features/repos/useRepos";
import { useSearch } from "@/features/search/useSearch";
import { ResultRow } from "@/features/search/ResultRow";
import { Button } from "@/components/ui/button";
import { SectionLabel } from "@/components/ui/datasheet";
import { InlineError } from "@/components/ui/inline-error";
import { describeError } from "@/lib/errors";

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
      <SectionLabel>search</SectionLabel>
      <div className="mb-5 flex items-center gap-2">
        <input
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") submit();
          }}
          placeholder="search code by meaning…"
          aria-label="search query"
          className="h-8 flex-1 rounded-md border border-input bg-card px-2.5 font-mono text-sm outline-none placeholder:text-muted-foreground focus-visible:ring-2 focus-visible:ring-ring"
        />
        <select
          value={repoId}
          aria-label="repository to search"
          onChange={(e) => {
            const next = new URLSearchParams(params);
            next.set("repo", e.target.value);
            setParams(next);
          }}
          className="h-8 rounded-md border border-input bg-card px-2.5 text-sm outline-none focus-visible:ring-2 focus-visible:ring-ring"
        >
          <option value="">select repo</option>
          {repos.map((r) => (
            <option key={r.id} value={r.id}>
              {r.name}
            </option>
          ))}
        </select>
        <Button size="default" disabled={!repoPath || draft.trim() === ""} onClick={submit}>
          search
        </Button>
      </div>

      {!repoPath ? (
        <p className="text-sm text-muted-foreground">select a repository to search its code.</p>
      ) : search.isLoading ? (
        <p className="text-sm text-muted-foreground">searching…</p>
      ) : search.isError ? (
        <InlineError message={describeError(search.error, "search failed")} onRetry={() => search.refetch()} />
      ) : submittedQuery && results.length === 0 ? (
        <p className="text-sm text-muted-foreground">
          no results for <span className="font-mono text-foreground">{submittedQuery}</span>
        </p>
      ) : results.length > 0 ? (
        <>
          <div className="mb-2 text-[0.6875rem] font-medium uppercase tracking-[0.13em] text-label">
            {results.length} {results.length === 1 ? "result" : "results"}
          </div>
          <div className="divide-y divide-border overflow-hidden rounded-md border border-border bg-card">
            {results.map((hit) => (
              <ResultRow key={hit.id} hit={hit} repoPath={repoPath} />
            ))}
          </div>
        </>
      ) : (
        <p className="text-sm text-muted-foreground">
          enter a query to search <span className="font-mono text-foreground">{selectedRepo?.name}</span>.
        </p>
      )}
    </section>
  );
}
