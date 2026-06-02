import { type HTMLAttributes, type ReactNode, forwardRef } from "react";
import { cn } from "@/lib/utils";

/**
 * Datasheet: a hairline-framed, hairline-separated list of rows. This is the
 * instrument's primary list affordance and replaces the old card-per-item
 * lists. Rows share one frame and one set of dividers, so a list of 50 reads
 * as one calm table rather than 50 floating cards.
 */
export const DataSheet = forwardRef<HTMLDivElement, HTMLAttributes<HTMLDivElement>>(
  ({ className, ...props }, ref) => (
    <div
      ref={ref}
      className={cn(
        "divide-y divide-border overflow-hidden rounded-md border border-border bg-card",
        className,
      )}
      {...props}
    />
  ),
);
DataSheet.displayName = "DataSheet";

/** A single datasheet row. Defaults to a comfortable instrument density. */
export const Row = forwardRef<HTMLDivElement, HTMLAttributes<HTMLDivElement>>(
  ({ className, ...props }, ref) => (
    <div
      ref={ref}
      className={cn("flex items-center gap-3 px-3.5 py-2.5", className)}
      {...props}
    />
  ),
);
Row.displayName = "Row";

/**
 * Section eyebrow. Small all-caps sans label with optional trailing count.
 * Counts render in mono so digits align with the data below.
 */
export function SectionLabel({
  children,
  count,
  className,
  as: Tag = "h2",
}: {
  children: ReactNode;
  count?: ReactNode;
  className?: string;
  as?: "h1" | "h2" | "h3";
}) {
  return (
    <Tag
      className={cn(
        "mb-3 flex items-baseline gap-2 text-[0.6875rem] font-medium uppercase tracking-[0.13em] text-label",
        className,
      )}
    >
      <span>{children}</span>
      {count != null ? (
        <span className="font-mono tracking-normal text-muted-foreground">{count}</span>
      ) : null}
    </Tag>
  );
}

/**
 * Label/value pair for readouts (overview vitals, repo stats). The value runs
 * in mono with tabular numerals so columns align across rows.
 */
export function Field({
  label,
  value,
  className,
}: {
  label: ReactNode;
  value: ReactNode;
  className?: string;
}) {
  return (
    <div className={cn("flex flex-col gap-1", className)}>
      <dt className="text-[0.625rem] font-medium uppercase tracking-[0.12em] text-label">{label}</dt>
      <dd className="font-mono text-foreground">{value}</dd>
    </div>
  );
}
