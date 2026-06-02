import { type HTMLAttributes } from "react";
import { cn } from "@/lib/utils";

/** Loading placeholder. Pulses gently; collapses to static under reduced-motion. */
export function Skeleton({ className, ...props }: HTMLAttributes<HTMLDivElement>) {
  return (
    <div
      aria-hidden
      className={cn("rounded bg-muted motion-safe:animate-pulse", className)}
      {...props}
    />
  );
}
