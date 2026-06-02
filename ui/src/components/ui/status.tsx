import { cn } from "@/lib/utils";

/**
 * The signal-only state vocabulary. Color encodes state, but never alone:
 * each state also has a distinct SHAPE and an accessible text label, so the
 * meaning survives color-blindness and greyscale.
 *
 *   ok   — filled disc        (healthy, indexed, succeeded, bound)
 *   run  — disc + pinging ring (indexing, in-progress)
 *   fail — solid triangle      (failed, unreachable)
 *   idle — hollow ring         (idle, never-indexed, unbound)
 */
export type StatusState = "ok" | "run" | "fail" | "idle";

const TONE: Record<StatusState, string> = {
  ok: "text-ok",
  run: "text-run",
  fail: "text-fail",
  idle: "text-idle",
};

const DEFAULT_NAME: Record<StatusState, string> = {
  ok: "ok",
  run: "running",
  fail: "failed",
  idle: "idle",
};

function Mark({ state }: { state: StatusState }) {
  if (state === "fail") {
    return (
      <svg viewBox="0 0 12 12" className="h-2.5 w-2.5 shrink-0" aria-hidden>
        <path d="M6 1.4 11.1 10.6H0.9z" fill="currentColor" />
      </svg>
    );
  }
  if (state === "idle") {
    return (
      <svg viewBox="0 0 12 12" className="h-2.5 w-2.5 shrink-0" aria-hidden>
        <circle cx="6" cy="6" r="3.4" fill="none" stroke="currentColor" strokeWidth="1.6" />
      </svg>
    );
  }
  if (state === "run") {
    return (
      <span className="relative inline-flex h-2.5 w-2.5 shrink-0 items-center justify-center" aria-hidden>
        <span className="absolute inline-flex h-full w-full rounded-full border border-current opacity-70 motion-safe:animate-ping" />
        <span className="relative inline-flex h-1.5 w-1.5 rounded-full bg-current" />
      </span>
    );
  }
  return (
    <svg viewBox="0 0 12 12" className="h-2.5 w-2.5 shrink-0" aria-hidden>
      <circle cx="6" cy="6" r="4" fill="currentColor" />
    </svg>
  );
}

type StatusGlyphProps = {
  state: StatusState;
  /** Visible text rendered beside the mark; becomes the accessible name. */
  label?: string;
  /** Accessible name when no visible label is shown. Defaults to the state name. */
  srLabel?: string;
  className?: string;
};

export function StatusGlyph({ state, label, srLabel, className }: StatusGlyphProps) {
  const name = srLabel ?? DEFAULT_NAME[state];
  return (
    <span
      className={cn("inline-flex items-center gap-1.5 leading-none", TONE[state], className)}
      role={label ? undefined : "img"}
      aria-label={label ? undefined : name}
    >
      <Mark state={state} />
      {label ? <span>{label}</span> : null}
    </span>
  );
}
