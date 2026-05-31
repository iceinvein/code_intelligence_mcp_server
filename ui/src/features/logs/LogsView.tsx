import { useLogStream } from "@/features/logs/useLogStream";

export function LogsView() {
  const { lines, connected } = useLogStream();
  return (
    <section>
      <h2 className="mb-3 flex items-center gap-2 text-[10px] uppercase tracking-[0.18em] text-label">
        logs
        <span className={connected ? "text-primary" : "text-muted-foreground"}>
          {connected ? "live" : "reconnecting"}
        </span>
      </h2>
      <div className="flex flex-col gap-0.5 font-mono text-[11px] leading-relaxed">
        {lines.length === 0 ? (
          <span className="text-muted-foreground">waiting for log output...</span>
        ) : (
          lines.map((line) => (
            <span key={line.id} className={line.lagged ? "text-destructive" : "text-foreground"}>
              {line.text}
            </span>
          ))
        )}
      </div>
    </section>
  );
}
