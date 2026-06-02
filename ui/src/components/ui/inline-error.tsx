import { StatusGlyph } from "@/components/ui/status";
import { Button } from "@/components/ui/button";

/**
 * Consistent failure surface: a calm hairline panel (not bare red text) with a
 * fail glyph and an optional recovery action. Use for query/load errors so
 * recovery is always one click away.
 */
export function InlineError({ message, onRetry }: { message: string; onRetry?: () => void }) {
  return (
    <div className="flex items-start gap-3 rounded-md border border-destructive/40 bg-destructive/5 px-4 py-3 text-sm">
      <StatusGlyph state="fail" srLabel="error" className="mt-0.5 shrink-0" />
      <p className="min-w-0 flex-1 text-foreground">{message}</p>
      {onRetry ? (
        <Button size="sm" variant="outline" onClick={onRetry} className="shrink-0">
          retry
        </Button>
      ) : null}
    </div>
  );
}
