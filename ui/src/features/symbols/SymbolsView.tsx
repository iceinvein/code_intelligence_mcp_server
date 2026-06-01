import { useEffect } from "react";
import { useSearchParams } from "react-router";
import { useRepos } from "@/features/repos/useRepos";
import { FileTree } from "@/features/symbols/FileTree";
import { SymbolInspector } from "@/features/symbols/SymbolInspector";
import { SymbolOutline } from "@/features/symbols/SymbolOutline";
import { useFileSymbols, useFiles } from "@/features/symbols/useSymbols";

export function SymbolsView() {
  const [params, setParams] = useSearchParams();
  const reposQuery = useRepos();
  const repos = reposQuery.data?.repos ?? [];

  const repoId = params.get("repo") ?? "";
  const selectedFile = params.get("file");
  const selectedSym = params.get("sym");

  useEffect(() => {
    if (!repoId && repos.length === 1) {
      setParams(
        (prev) => {
          const next = new URLSearchParams(prev);
          next.set("repo", repos[0]!.id);
          return next;
        },
        { replace: true },
      );
    }
  }, [repoId, repos, setParams]);

  const selectedRepo = repos.find((r) => r.id === repoId) ?? null;
  const repoPath = selectedRepo?.path ?? null;

  const files = useFiles(repoPath);
  const fileSymbols = useFileSymbols(repoPath, selectedFile);

  const setParam = (key: string, value: string | null) => {
    const next = new URLSearchParams(params);
    if (value) next.set(key, value);
    else next.delete(key);
    return next;
  };

  const onSelectFile = (path: string) => {
    const next = setParam("file", path);
    next.delete("sym");
    setParams(next);
  };

  const onSelectSymbol = (name: string) => setParams(setParam("sym", name));

  return (
    <section>
      <div className="mb-3 flex items-center gap-2">
        <h2 className="text-[10px] uppercase tracking-[0.18em] text-label">symbols</h2>
        <select
          value={repoId}
          aria-label="repository to browse"
          onChange={(e) => {
            const next = setParam("repo", e.target.value);
            next.delete("file");
            next.delete("sym");
            setParams(next);
          }}
          className="ml-auto h-7 rounded-md border border-border bg-card px-2 text-[12px] outline-none focus-visible:ring-2 focus-visible:ring-ring"
        >
          <option value="">select repo</option>
          {repos.map((r) => (
            <option key={r.id} value={r.id}>
              {r.name}
            </option>
          ))}
        </select>
      </div>

      {!repoPath ? (
        <div className="text-xs text-muted-foreground">select a repository to browse</div>
      ) : (
        <div className="flex flex-col gap-3 lg:flex-row">
          <div className="max-h-[70vh] min-h-32 overflow-auto border-b border-border pb-2 lg:w-56 lg:shrink-0 lg:border-b-0 lg:border-r lg:pb-0 lg:pr-2">
            {files.isLoading ? (
              <div className="text-[11px] text-muted-foreground">loading files...</div>
            ) : files.isError ? (
              <div className="text-[11px] text-destructive">failed to load files</div>
            ) : (
              <FileTree
                files={files.data?.files ?? []}
                selectedFile={selectedFile}
                onSelectFile={onSelectFile}
              />
            )}
          </div>

          <div className="max-h-[70vh] min-h-32 overflow-auto border-b border-border pb-2 lg:w-56 lg:shrink-0 lg:border-b-0 lg:border-r lg:pb-0 lg:pr-2">
            {!selectedFile ? (
              <div className="text-[11px] text-muted-foreground">select a file</div>
            ) : fileSymbols.isLoading ? (
              <div className="text-[11px] text-muted-foreground">loading symbols...</div>
            ) : fileSymbols.isError ? (
              <div className="text-[11px] text-destructive">failed to load symbols</div>
            ) : (
              <SymbolOutline
                symbols={fileSymbols.data?.symbols ?? []}
                selectedName={selectedSym}
                onSelect={onSelectSymbol}
              />
            )}
          </div>

          <div className="min-w-0 flex-1">
            {selectedFile && selectedSym ? (
              <SymbolInspector repoPath={repoPath} symbolName={selectedSym} file={selectedFile} />
            ) : (
              <div className="text-[11px] text-muted-foreground">select a symbol to inspect</div>
            )}
          </div>
        </div>
      )}
    </section>
  );
}
