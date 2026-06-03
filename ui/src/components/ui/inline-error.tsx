import { StatusGlyph } from "@/components/ui/status";
import { Button } from "@/components/ui/button";

/**
 * Consistent failure surface with a built-in recovery action.
 *
 * Default: a calm hairline panel (not bare red text) with a fail glyph and a
 * retry button. Use for top-level / full-width load errors.
 *
 * `compact`: a single muted line with an inline retry link. Use inside narrow
 * panes or nested panels (symbol columns, definition/references) where a full
 * panel would be too heavy.
 */
export function InlineError({
  message,
  onRetry,
  compact,
}: {
  message: string;
  onRetry?: () => void;
  compact?: boolean;
}) {
  if (compact) {
    return (
      <div className="flex flex-wrap items-center gap-x-2 gap-y-0.5 text-[0.6875rem] text-destructive">
        <span>{message}</span>
        {onRetry ? (
          <button
            type="button"
            onClick={onRetry}
            className="text-muted-foreground underline-offset-2 hover:text-foreground hover:underline"
          >
            retry
          </button>
        ) : null}
      </div>
    );
  }
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
