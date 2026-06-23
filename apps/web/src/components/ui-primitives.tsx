import { Link } from "@tanstack/react-router"
import { ChevronRight, Search } from "lucide-react"
import type { LucideIcon } from "lucide-react"
import type { ComponentProps, ReactNode } from "react"

import {
  Card,
  CornerMarkers,
} from "@workspace/ui/components/card"
import { Input } from "@workspace/ui/components/input"
import { cn } from "@workspace/ui/lib/utils"

export function IconTile({
  icon: Icon,
  size = "md",
  className,
  iconClassName,
}: {
  icon: LucideIcon
  size?: "sm" | "md" | "lg"
  className?: string
  iconClassName?: string
}) {
  const box =
    size === "sm" ? "size-6" : size === "lg" ? "size-10" : "size-8"
  const iconSize =
    size === "sm" ? "size-3" : size === "lg" ? "size-5" : "size-4"

  return (
    <div
      className={cn(
        "grid shrink-0 place-items-center border border-divider bg-input",
        box,
        className
      )}
    >
      <Icon
        className={cn(iconSize, "text-muted-foreground", iconClassName)}
      />
    </div>
  )
}

export function InsetDivider({
  side = "bottom",
  className,
}: {
  side?: "top" | "bottom"
  className?: string
}) {
  return (
    <span
      aria-hidden
      className={cn(
        "pointer-events-none absolute inset-x-3 h-px bg-divider",
        side === "top" ? "top-0" : "bottom-0",
        className
      )}
    />
  )
}

export function SectionLabel({ children }: { children: ReactNode }) {
  return <p className="text-label px-3 pt-3 pb-2">{children}</p>
}

type PanelProps = {
  children: ReactNode
  className?: string
  markers?: boolean
}

export function Panel({
  children,
  className,
  markers = true,
}: PanelProps) {
  return (
    <Card markers={markers} className={cn("gap-0 overflow-hidden", className)}>
      {children}
    </Card>
  )
}

export function PanelHeader({
  title,
  description,
  action,
  icon,
}: {
  title: string
  description?: string
  action?: ReactNode
  icon?: LucideIcon
}) {
  const HeaderIcon = icon

  return (
    <div className="relative flex items-center gap-3 px-5 py-4">
      {HeaderIcon ? <IconTile icon={HeaderIcon} /> : null}
      <div className="min-w-0 flex-1">
        <h2 className="truncate text-lg font-semibold tracking-tight text-foreground">
          {title}
        </h2>
        {description ? (
          <p className="mt-0.5 truncate text-sm text-muted-foreground">
            {description}
          </p>
        ) : null}
      </div>
      {action ? <div className="shrink-0">{action}</div> : null}
      <InsetDivider />
    </div>
  )
}

export function PageHeader({
  eyebrow = "/// Section",
  title,
  action,
  search,
  icon,
}: {
  eyebrow?: string
  title: string
  action?: ReactNode
  search?: ReactNode
  icon?: LucideIcon
}) {
  const HeaderIcon = icon

  return (
    <header
      className="relative flex shrink-0 items-center gap-5 bg-card px-5"
      style={{ height: "var(--token-header-height)" }}
    >
      <CornerMarkers />
      {HeaderIcon ? <IconTile icon={HeaderIcon} size="sm" /> : null}
      <div className="flex min-w-0 shrink-0 flex-col gap-0.5">
        <span className="text-label text-muted-foreground">{eyebrow}</span>
        <h1 className="truncate text-lg font-semibold tracking-tight text-foreground">
          {title}
        </h1>
      </div>
      {search ? (
        <div className="mx-auto hidden min-w-0 flex-1 justify-center md:flex">
          {search}
        </div>
      ) : null}
      {action ? <div className="ml-auto shrink-0">{action}</div> : null}
      <InsetDivider />
    </header>
  )
}

export function SearchField({
  placeholder,
  className,
  ...props
}: ComponentProps<typeof Input>) {
  return (
    <div className="relative w-full">
      <Search
        aria-hidden
        className="pointer-events-none absolute top-1/2 left-2.5 size-3.5 -translate-y-1/2 text-faint"
      />
      <Input
        type="search"
        placeholder={placeholder}
        className={cn("h-8 max-w-md bg-input pl-8 text-sm", className)}
        {...props}
      />
    </div>
  )
}

export function ActionLink({
  to,
  children,
  className,
}: {
  to: string
  children: ReactNode
  className?: string
}) {
  return (
    <Link
      to={to}
      className={cn(
        "inline-flex items-center gap-1 text-xs uppercase tracking-wider text-muted-foreground transition-colors hover:text-signal",
        className
      )}
    >
      {children}
      <ChevronRight className="size-3" />
    </Link>
  )
}

export function EmptyState({
  icon: Icon,
  message,
}: {
  icon: LucideIcon
  message: string
}) {
  return (
    <div className="flex flex-col items-center gap-3 px-5 py-10 text-center">
      <IconTile icon={Icon} size="lg" iconClassName="text-faint" />
      <p className="max-w-sm text-sm text-muted-foreground">{message}</p>
    </div>
  )
}

export function TableHeadLabel({
  icon: Icon,
  children,
}: {
  icon: LucideIcon
  children: ReactNode
}) {
  return (
    <span className="inline-flex items-center gap-1.5">
      <Icon className="size-3.5 text-faint" />
      {children}
    </span>
  )
}

export function FeedList({ children }: { children: ReactNode }) {
  return <ul>{children}</ul>
}

export function FeedItem({
  icon: Icon,
  title,
  meta,
  description,
  action,
}: {
  icon?: LucideIcon
  title: ReactNode
  meta?: string
  description?: ReactNode
  action?: ReactNode
}) {
  return (
    <li className="relative flex gap-3 px-5 py-3.5 transition-colors hover:bg-muted/30">
      {Icon ? <IconTile icon={Icon} /> : null}
      <div className="min-w-0 flex-1">
        <div className="flex flex-wrap items-baseline gap-x-2 gap-y-0.5">
          <div className="text-sm font-medium text-foreground">{title}</div>
          {meta ? <span className="text-xs text-faint">{meta}</span> : null}
        </div>
        {description ? (
          <p className="mt-1 text-sm leading-snug text-muted-foreground">
            {description}
          </p>
        ) : null}
      </div>
      {action ? <div className="shrink-0 self-center">{action}</div> : null}
      <InsetDivider />
    </li>
  )
}

export { CornerMarkers }