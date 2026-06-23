import * as React from "react"
import { cva, type VariantProps } from "class-variance-authority"
import { Slot } from "radix-ui"

import { cn } from "@workspace/ui/lib/utils"

const badgeVariants = cva(
  "inline-flex h-5 w-fit max-w-full min-w-0 shrink-0 items-center gap-1 overflow-hidden px-2 py-0.5 text-xs font-medium whitespace-nowrap",
  {
    variants: {
      variant: {
        default: "bg-secondary text-secondary-foreground",
        secondary: "bg-muted text-foreground",
        destructive: "bg-destructive/20 text-destructive",
        outline: "border border-border bg-transparent text-foreground",
        ghost: "bg-transparent text-muted-foreground",
        accent: "bg-accent text-accent-foreground",
        warning: "bg-chart-4/20 text-chart-4",
        link: "bg-transparent text-signal",
      },
    },
    defaultVariants: {
      variant: "default",
    },
  }
)

function Badge({
  className,
  variant = "default",
  asChild = false,
  children,
  ...props
}: React.ComponentProps<"span"> &
  VariantProps<typeof badgeVariants> & { asChild?: boolean }) {
  const Comp = asChild ? Slot.Root : "span"

  return (
    <Comp
      data-slot="badge"
      data-variant={variant}
      className={cn(badgeVariants({ variant }), className)}
      {...props}
    >
      {children}
    </Comp>
  )
}

export { Badge, badgeVariants }