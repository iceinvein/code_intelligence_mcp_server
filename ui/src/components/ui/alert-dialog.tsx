import { AlertDialog as Primitive } from "@base-ui/react/alert-dialog";
import { type ComponentPropsWithoutRef, type ReactNode } from "react";
import { cn } from "@/lib/utils";
import { buttonVariants } from "@/components/ui/button";

export const AlertDialog = Primitive.Root;
export const AlertDialogTrigger = Primitive.Trigger;

export function AlertDialogContent({
  className,
  children,
  ...props
}: ComponentPropsWithoutRef<typeof Primitive.Popup>) {
  return (
    <Primitive.Portal>
      <Primitive.Backdrop className="fixed inset-0 z-50 bg-[oklch(20%_0.025_250/0.45)]" />
      <Primitive.Popup
        className={cn(
          "fixed left-1/2 top-1/2 z-50 w-[min(440px,90vw)] -translate-x-1/2 -translate-y-1/2 rounded-lg border border-border bg-popover p-5 text-popover-foreground shadow-[0_12px_40px_-16px_oklch(20%_0.02_250/0.45)]",
          className,
        )}
        {...props}
      >
        {children}
      </Primitive.Popup>
    </Primitive.Portal>
  );
}

export function AlertDialogHeader({
  className,
  ...props
}: {
  className?: string;
  children?: ReactNode;
}) {
  return <div className={cn("flex flex-col gap-2", className)} {...props} />;
}

export function AlertDialogFooter({
  className,
  ...props
}: {
  className?: string;
  children?: ReactNode;
}) {
  return <div className={cn("mt-5 flex justify-end gap-2", className)} {...props} />;
}

export function AlertDialogTitle({
  className,
  ...props
}: ComponentPropsWithoutRef<typeof Primitive.Title>) {
  return <Primitive.Title className={cn("text-sm font-medium", className)} {...props} />;
}

export function AlertDialogDescription({
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

export function AlertDialogAction({
  className,
  ...props
}: ComponentPropsWithoutRef<typeof Primitive.Close>) {
  return (
    <Primitive.Close
      className={cn(buttonVariants({ variant: "destructive" }), className)}
      {...props}
    />
  );
}

export function AlertDialogCancel({
  className,
  ...props
}: ComponentPropsWithoutRef<typeof Primitive.Close>) {
  return (
    <Primitive.Close
      className={cn(buttonVariants({ variant: "outline" }), className)}
      {...props}
    />
  );
}
