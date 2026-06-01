import type { ConsentDecision } from "@/api/consent";
import { Button } from "@/components/ui/button";
import { useConsent, useResolveConsent } from "@/features/consent/useConsent";

export function ConsentView() {
  const consent = useConsent();
  const resolve = useResolveConsent();
  const pending = consent.data?.pending ?? [];
  const declined = consent.data?.declined ?? [];

  const act = (repo: string, decision: ConsentDecision) => resolve.mutate({ repo, decision });

  return (
    <section>
      <h2 className="mb-3 text-[10px] uppercase tracking-[0.18em] text-label">consent</h2>

      {consent.isLoading ? (
        <div className="text-xs text-muted-foreground">loading...</div>
      ) : consent.isError ? (
        <div className="text-xs text-destructive">
          failed to load consent: {String((consent.error as Error).message)}
        </div>
      ) : pending.length === 0 && declined.length === 0 ? (
        <div className="max-w-prose text-xs text-muted-foreground">
          No repositories are awaiting a decision. When an agent binds a never-indexed repo
          implicitly (for example a git worktree or a temp copy), it appears here so you can
          approve or decline indexing before it runs.
        </div>
      ) : (
        <>
          <div className="mb-2 text-[10px] uppercase tracking-[0.18em] text-label">
            pending &middot; {pending.length}
          </div>
          {pending.length === 0 ? (
            <div className="mb-6 text-xs text-muted-foreground">nothing pending</div>
          ) : (
            <div className="mb-6">
              {pending.map((p) => (
                <div key={p.repo_id} className="border-b border-border py-2">
                  <div className="flex items-baseline gap-3">
                    <span className="font-mono text-[12px] text-primary">{p.repo_path}</span>
                    <span className="text-[10px] uppercase tracking-wide text-muted-foreground">
                      {p.detected}
                    </span>
                    <div className="ml-auto flex gap-2">
                      <Button
                        size="sm"
                        disabled={resolve.isPending}
                        onClick={() => act(p.repo_path, "approve")}
                      >
                        approve
                      </Button>
                      <Button
                        size="sm"
                        variant="outline"
                        disabled={resolve.isPending}
                        onClick={() => act(p.repo_path, "decline")}
                      >
                        decline
                      </Button>
                    </div>
                  </div>
                  <div className="mt-1 text-[11px] text-muted-foreground">
                    {p.recommendation}
                    {p.detail ? ` / ${p.detail}` : ""}
                  </div>
                </div>
              ))}
            </div>
          )}

          {declined.length > 0 ? (
            <>
              <div className="mb-2 text-[10px] uppercase tracking-[0.18em] text-label">
                previously declined &middot; {declined.length}
              </div>
              <div>
                {declined.map((d) => (
                  <div
                    key={d.repo_id}
                    className="flex items-baseline gap-3 border-b border-border py-2"
                  >
                    <span className="font-mono text-[12px] text-foreground">{d.repo_path}</span>
                    <span className="text-[10px] uppercase tracking-wide text-muted-foreground">
                      {d.detected}
                    </span>
                    <Button
                      size="sm"
                      variant="outline"
                      className="ml-auto"
                      disabled={resolve.isPending}
                      onClick={() => act(d.repo_path, "approve")}
                    >
                      re-approve
                    </Button>
                  </div>
                ))}
              </div>
            </>
          ) : null}
        </>
      )}
    </section>
  );
}
