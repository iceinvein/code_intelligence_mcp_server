import { useEffect, useRef, useState } from "react";
import { Folder, GitBranch } from "lucide-react";
import { useFsList } from "@/features/repos/useFsList";
import { useAddRepo } from "@/features/repos/useRepos";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogFooter,
} from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";

function crumbs(path: string): { label: string; path: string }[] {
  const parts = path.split("/").filter(Boolean);
  const acc: { label: string; path: string }[] = [{ label: "/", path: "/" }];
  let cur = "";
  for (const p of parts) {
    cur += `/${p}`;
    acc.push({ label: p, path: cur });
  }
  return acc;
}

export function FolderPickerDialog({
  open,
  onOpenChange,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const [currentPath, setCurrentPath] = useState<string | undefined>(undefined);
  const [showHidden, setShowHidden] = useState(false);
  const lastGoodPath = useRef<string>("");
  const list = useFsList(currentPath, showHidden, open);
  const add = useAddRepo();

  useEffect(() => {
    if (open) {
      setCurrentPath(undefined);
      setShowHidden(false);
      add.reset();
    }
  }, [open]);

  const listing = list.data;
  useEffect(() => {
    if (listing?.path) lastGoodPath.current = listing.path;
  }, [listing?.path]);

  const displayPath = listing?.path ?? lastGoodPath.current;
  const submit = () => {
    if (!displayPath) return;
    add.mutate(displayPath, { onSuccess: () => onOpenChange(false) });
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Select a repository folder</DialogTitle>
        </DialogHeader>

        <div className="mt-2 flex items-center justify-between gap-2 text-[11px] text-muted-foreground">
          <div className="flex flex-wrap items-center gap-1 font-mono">
            {crumbs(displayPath || "/").map((c, i) => (
              <button
                key={c.path}
                className="hover:text-foreground"
                onClick={() => setCurrentPath(c.path)}
              >
                {i === 0 ? c.label : `/${c.label}`}
              </button>
            ))}
          </div>
          <label className="flex shrink-0 items-center gap-1">
            <input
              type="checkbox"
              checked={showHidden}
              onChange={(e) => setShowHidden(e.target.checked)}
              aria-label="show hidden folders"
            />
            hidden
          </label>
        </div>

        <div className="mt-2 min-h-[200px] flex-1 overflow-auto rounded-md border border-border">
          {list.isError ? (
            <div className="p-3 text-[12px] text-destructive">
              cannot open this folder: {String((list.error as Error).message)}
              <button
                className="ml-2 underline"
                onClick={() => setCurrentPath(lastGoodPath.current || undefined)}
              >
                go back
              </button>
            </div>
          ) : list.isLoading && !listing ? (
            <div className="p-3 text-[12px] text-muted-foreground">loading...</div>
          ) : listing && listing.entries.length === 0 ? (
            <div className="p-3 text-[12px] text-muted-foreground">no subfolders</div>
          ) : (
            <ul>
              {listing?.entries.map((entry) => (
                <li key={entry.path}>
                  <button
                    className={`flex w-full items-center gap-2 px-3 py-1 text-left text-[12px] hover:bg-accent hover:text-accent-foreground ${
                      entry.hidden ? "text-muted-foreground" : ""
                    }`}
                    onClick={() => setCurrentPath(entry.path)}
                  >
                    <Folder className="h-3.5 w-3.5 shrink-0" aria-hidden />
                    <span className="truncate font-mono">{entry.name}</span>
                    {entry.has_git ? (
                      <span className="ml-auto inline-flex items-center gap-1 rounded bg-primary/15 px-1 text-[10px] text-primary">
                        <GitBranch className="h-3 w-3" aria-hidden />
                        git
                      </span>
                    ) : null}
                  </button>
                </li>
              ))}
            </ul>
          )}
        </div>

        <DialogFooter>
          <span className="min-w-0 flex-1 truncate font-mono text-[11px] text-muted-foreground">
            {displayPath}
          </span>
          {add.isError ? (
            <span className="text-[11px] text-destructive">
              {String((add.error as Error).message)}
            </span>
          ) : null}
          <Button size="sm" disabled={!displayPath || list.isLoading || add.isPending} onClick={submit}>
            {add.isPending ? "adding" : "Add this folder"}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
