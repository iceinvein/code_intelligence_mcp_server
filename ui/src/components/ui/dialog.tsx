import { Dialog as Primitive } from "@base-ui/react/dialog";
import { type ComponentPropsWithoutRef, type ReactNode } from "react";
import { cn } from "@/lib/utils";

export const Dialog = Primitive.Root;
export const DialogTrigger = Primitive.Trigger;
export const DialogClose = Primitive.Close;

export function DialogContent({
  className,
  children,
  ...props
}: ComponentPropsWithoutRef<typeof Primitive.Popup>) {
  return (
    <Primitive.Portal>
      <Primitive.Backdrop className="fixed inset-0 z-50 bg-[oklch(20%_0.025_250/0.45)]" />
      <Primitive.Popup
        className={cn(
          "fixed left-1/2 top-1/2 z-50 flex max-h-[80vh] w-[min(560px,92vw)] -translate-x-1/2 -translate-y-1/2 flex-col rounded-lg border border-border bg-popover p-5 text-popover-foreground shadow-[0_12px_40px_-16px_oklch(20%_0.02_250/0.45)]",
          className,
        )}
        {...props}
      >
        {children}
      </Primitive.Popup>
    </Primitive.Portal>
  );
}

export function DialogHeader({
  className,
  ...props
}: {
  className?: string;
  children?: ReactNode;
}) {
  return <div className={cn("flex flex-col gap-1", className)} {...props} />;
}

export function DialogFooter({
  className,
  ...props
}: {
  className?: string;
  children?: ReactNode;
}) {
  return <div className={cn("mt-3 flex items-center justify-between gap-2", className)} {...props} />;
}

export function DialogTitle({
  className,
  ...props
}: ComponentPropsWithoutRef<typeof Primitive.Title>) {
  return <Primitive.Title className={cn("text-sm font-medium", className)} {...props} />;
}

export function DialogDescription({
  className,
  ...props
}: ComponentPropsWithoutRef<typeof Primitive.Description>) {
  return (
    <Primitive.Description
      className={cn("text-xs text-muted-foreground", className)}
      {...props}
    />
  );
}
