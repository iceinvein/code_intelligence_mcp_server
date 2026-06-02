import { useRender } from "@base-ui/react/use-render";
import { cva, type VariantProps } from "class-variance-authority";
import { type ButtonHTMLAttributes, type ReactElement, forwardRef } from "react";
import { cn } from "@/lib/utils";

const buttonVariants = cva(
  "inline-flex select-none items-center justify-center gap-2 whitespace-nowrap rounded-md text-xs font-medium transition-[color,background-color,border-color,opacity] duration-150 disabled:pointer-events-none disabled:opacity-50",
  {
    variants: {
      variant: {
        default: "bg-primary text-primary-foreground hover:opacity-90",
        outline: "border border-input bg-transparent text-foreground hover:bg-muted",
        ghost: "text-foreground hover:bg-muted",
        destructive: "border border-destructive text-destructive hover:bg-destructive/10",
      },
      size: {
        default: "h-7 px-3 py-1",
        sm: "h-6 px-2.5",
        icon: "h-7 w-7",
      },
    },
    defaultVariants: { variant: "default", size: "default" },
  },
);

type ButtonProps = ButtonHTMLAttributes<HTMLButtonElement> &
  VariantProps<typeof buttonVariants> & {
    /** Base UI render prop: replace the underlying <button> (e.g. with a router Link). */
    render?: ReactElement;
  };

export const Button = forwardRef<HTMLButtonElement, ButtonProps>(
  ({ className, variant, size, render, ...props }, ref) => {
    return useRender({
      defaultTagName: "button",
      render,
      props: { ref, className: cn(buttonVariants({ variant, size, className })), ...props },
    });
  },
);
Button.displayName = "Button";

export { buttonVariants };
