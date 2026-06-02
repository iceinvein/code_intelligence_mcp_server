import { useLogStream } from "@/features/logs/useLogStream";
import { StatusGlyph } from "@/components/ui/status";

export function LogsView() {
  const { lines, connected } = useLogStream();
  return (
    <section>
      <div className="mb-3 flex items-center gap-3">
        <span className="text-[0.6875rem] font-medium uppercase tracking-[0.13em] text-label">
          logs
        </span>
        <StatusGlyph
          state={connected ? "ok" : "run"}
          label={connected ? "live" : "reconnecting"}
          className="text-[0.6875rem]"
        />
      </div>
      <div
        role="log"
        aria-live="polite"
        aria-label="daemon log stream"
        className="flex flex-col gap-0.5 overflow-x-auto rounded-md border border-border bg-card px-3.5 py-3 font-mono text-[0.6875rem] leading-relaxed"
      >
        {lines.length === 0 ? (
          <span className="text-muted-foreground">waiting for log output…</span>
        ) : (
          lines.map((line) => (
            <span
              key={line.id}
              className={line.lagged ? "whitespace-pre text-fail" : "whitespace-pre text-foreground"}
            >
              {line.text}
            </span>
          ))
        )}
      </div>
    </section>
  );
}
