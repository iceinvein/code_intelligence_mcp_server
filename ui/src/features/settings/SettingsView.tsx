import { useMemo, useState } from "react";
import { fetchRepos, reindexRepo } from "@/api/repos";
import type { SettingValue } from "@/api/settings";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import { SettingField } from "@/features/settings/SettingField";
import { changedKeys, pendingChanges, type SettingsDraft } from "@/features/settings/diff";
import { usePutSettings, useSettings } from "@/features/settings/useSettings";

const HEAVY = ["embeddings_backend", "embeddings_device"];

export function SettingsView() {
  const settings = useSettings();
  const put = usePutSettings();
  const [draft, setDraft] = useState<SettingsDraft>({});
  const [confirm, setConfirm] = useState<{ key: string; value: SettingValue } | null>(null);
  const [saved, setSaved] = useState<{ needsReindex: boolean } | null>(null);
  const [reindexing, setReindexing] = useState(false);

  const fields = settings.data?.fields ?? [];
  const dirtyKeys = useMemo(() => changedKeys(fields, draft), [fields, draft]);

  const groups = useMemo(() => {
    const order: string[] = [];
    const map = new Map<string, typeof fields>();
    for (const f of fields) {
      if (!map.has(f.group)) {
        map.set(f.group, []);
        order.push(f.group);
      }
      map.get(f.group)!.push(f);
    }
    return order.map((group) => ({ group, fields: map.get(group)! }));
  }, [fields]);

  const commitChange = (key: string, value: SettingValue) =>
    setDraft((d) => ({ ...d, [key]: value }));

  const onChange = (key: string, value: SettingValue) => {
    setSaved(null);
    if (HEAVY.includes(key)) {
      setConfirm({ key, value });
      return;
    }
    commitChange(key, value);
  };

  const onReset = (key: string) => {
    const f = fields.find((x) => x.key === key);
    if (f) {
      setSaved(null);
      setDraft((d) => ({ ...d, [key]: f.default }));
    }
  };

  const onDiscard = () => {
    setDraft({});
    setSaved(null);
  };

  const onSave = () => {
    const changes = pendingChanges(fields, draft);
    const needsReindex = fields.some((f) => f.key in changes && f.needs_reindex);
    put.mutate(changes, {
      onSuccess: () => {
        setDraft({});
        setSaved({ needsReindex });
      },
    });
  };

  const reindexAll = async () => {
    setReindexing(true);
    try {
      const repos = await fetchRepos();
      await Promise.all(repos.repos.map((r) => reindexRepo(r.id)));
    } finally {
      setReindexing(false);
    }
  };

  return (
    <section className="pb-20">
      <h2 className="mb-3 text-[10px] uppercase tracking-[0.18em] text-label">settings</h2>

      {saved ? (
        <div className="mb-4 rounded-md border border-primary bg-primary/10 px-3 py-2 text-xs">
          <div className="text-foreground">
            Saved to <span className="font-mono">~/.code-intelligence/server.toml</span>. Restart
            the daemon to apply:{" "}
            <span className="font-mono text-primary">code-intelligence-mcp-server stop</span>{" "}
            (launchd respawns it).
          </div>
          {saved.needsReindex ? (
            <div className="mt-2 flex flex-wrap items-center gap-3">
              <span className="text-muted-foreground">
                Some changes need a reindex to take effect.
              </span>
              <Button size="sm" variant="outline" disabled={reindexing} onClick={reindexAll}>
                {reindexing ? "reindexing..." : "reindex affected repos"}
              </Button>
            </div>
          ) : null}
        </div>
      ) : null}

      <AlertDialog open={confirm !== null} onOpenChange={(open) => !open && setConfirm(null)}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Change a heavy setting?</AlertDialogTitle>
            <AlertDialogDescription>
              Changing <span className="font-mono">{confirm?.key}</span> alters the embeddings and
              requires a full reindex. The <span className="font-mono">hash</span> backend is a
              non-semantic test backend and will degrade search quality.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel onClick={() => setConfirm(null)}>cancel</AlertDialogCancel>
            <AlertDialogAction
              onClick={() => {
                if (confirm) commitChange(confirm.key, confirm.value);
                setConfirm(null);
              }}
            >
              change it
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>

      {settings.isLoading ? (
        <div className="text-xs text-muted-foreground">loading...</div>
      ) : settings.isError ? (
        <div className="text-xs text-destructive">
          failed to load settings: {String((settings.error as Error).message)}
        </div>
      ) : (
        <>
          {groups.map(({ group, fields: groupFields }) => (
            <div key={group} className="mb-6">
              <div className="mb-2 text-[10px] uppercase tracking-[0.18em] text-label">
                {group}
              </div>
              {groupFields.map((f) => (
                <SettingField
                  key={f.key}
                  field={f}
                  draftValue={draft[f.key]}
                  dirty={dirtyKeys.includes(f.key)}
                  onChange={onChange}
                  onReset={onReset}
                />
              ))}
            </div>
          ))}

          {put.isError ? (
            <div className="text-xs text-destructive">{String((put.error as Error).message)}</div>
          ) : null}

          {dirtyKeys.length > 0 ? (
            <div className="fixed bottom-0 left-0 right-0 border-t border-primary bg-background/95 px-4 py-3 sm:left-44 sm:px-6">
              <div className="flex items-center gap-3">
                <span className="text-xs text-foreground">
                  {dirtyKeys.length} unsaved change{dirtyKeys.length === 1 ? "" : "s"}
                </span>
                <div className="ml-auto flex gap-2">
                  <Button size="sm" variant="outline" disabled={put.isPending} onClick={onDiscard}>
                    discard
                  </Button>
                  <Button size="sm" disabled={put.isPending} onClick={onSave}>
                    {put.isPending ? "saving" : "save"}
                  </Button>
                </div>
              </div>
            </div>
          ) : null}
        </>
      )}
    </section>
  );
}
