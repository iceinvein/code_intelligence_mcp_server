import type { ConsentDecision } from "@/api/consent";
import { Button } from "@/components/ui/button";
import { DataSheet, Row, SectionLabel } from "@/components/ui/datasheet";
import { StatusGlyph } from "@/components/ui/status";
import { Skeleton } from "@/components/ui/skeleton";
import { InlineError } from "@/components/ui/inline-error";
import { describeError } from "@/lib/errors";
import { useConsent, useResolveConsent } from "@/features/consent/useConsent";

export function ConsentView() {
  const consent = useConsent();
  const resolve = useResolveConsent();
  const pending = consent.data?.pending ?? [];
  const declined = consent.data?.declined ?? [];

  const act = (repo: string, decision: ConsentDecision) => resolve.mutate({ repo, decision });

  return (
    <section className="flex flex-col gap-8">
      <SectionLabel className="mb-0">consent</SectionLabel>

      {consent.isLoading ? (
        <Skeleton className="h-24 w-full" />
      ) : consent.isError ? (
        <InlineError
          message={describeError(consent.error, "couldn't load consent")}
          onRetry={() => consent.refetch()}
        />
      ) : pending.length === 0 && declined.length === 0 ? (
        <div className="max-w-prose rounded-md border border-dashed border-border px-4 py-6 text-sm leading-relaxed text-muted-foreground">
          No repositories are awaiting a decision. When an agent binds a never-indexed repo
          implicitly (for example a git worktree or a temp copy), it appears here so you can approve
          or decline indexing before it runs.
        </div>
      ) : (
        <>
          <section>
            <SectionLabel as="h3" count={pending.length}>
              pending
            </SectionLabel>
            {pending.length === 0 ? (
              <p className="text-sm text-muted-foreground">nothing pending</p>
            ) : (
              <DataSheet>
                {pending.map((p) => (
                  <Row key={p.repo_id} className="flex-wrap">
                    <StatusGlyph state="run" srLabel="awaiting decision" />
                    <span className="min-w-0 flex-1 truncate font-mono text-[0.6875rem] text-foreground">
                      {p.repo_path}
                    </span>
                    <span className="shrink-0 text-[0.625rem] uppercase tracking-[0.1em] text-muted-foreground">
                      {p.detected}
                    </span>
                    <div className="flex shrink-0 gap-2">
                      <Button size="sm" disabled={resolve.isPending} onClick={() => act(p.repo_path, "approve")}>
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
                    {p.recommendation || p.detail ? (
                      <div className="w-full pl-[26px] text-[0.6875rem] text-muted-foreground">
                        {p.recommendation}
                        {p.detail ? ` · ${p.detail}` : ""}
                      </div>
                    ) : null}
                  </Row>
                ))}
              </DataSheet>
            )}
          </section>

          {declined.length > 0 ? (
            <section>
              <SectionLabel as="h3" count={declined.length}>
                previously declined
              </SectionLabel>
              <DataSheet>
                {declined.map((d) => (
                  <Row key={d.repo_id}>
                    <StatusGlyph state="idle" srLabel="declined" />
                    <span className="min-w-0 flex-1 truncate font-mono text-[0.6875rem] text-foreground">
                      {d.repo_path}
                    </span>
                    <span className="shrink-0 text-[0.625rem] uppercase tracking-[0.1em] text-muted-foreground">
                      {d.detected}
                    </span>
                    <Button
                      size="sm"
                      variant="outline"
                      disabled={resolve.isPending}
                      onClick={() => act(d.repo_path, "approve")}
                    >
                      re-approve
                    </Button>
                  </Row>
                ))}
              </DataSheet>
            </section>
          ) : null}
        </>
      )}
    </section>
  );
}
