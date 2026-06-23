import * as React from "react"

import { cn } from "@workspace/ui/lib/utils"

function Card({
  className,
  size = "default",
  markers = false,
  children,
  ...props
}: React.ComponentProps<"div"> & {
  size?: "default" | "sm"
  markers?: boolean
}) {
  return (
    <div
      data-slot="card"
      data-size={size}
      className={cn(
        "group/card relative flex flex-col gap-0 overflow-hidden border border-divider bg-card text-sm text-card-foreground [--card-spacing:--spacing(4)] data-[size=sm]:[--card-spacing:--spacing(3)]",
        className
      )}
      {...props}
    >
      {markers ? <CornerMarkers /> : null}
      {children}
    </div>
  )
}

function CornerMarkers({
  color = "var(--token-marker-color)",
  size,
}: {
  color?: string
  size?: number
}) {
  const markerSize = size ?? 12
  const positions = [
    { key: "tl", style: { top: 0, left: 0 }, borders: "border-t border-l" },
    { key: "tr", style: { top: 0, right: 0 }, borders: "border-t border-r" },
    {
      key: "bl",
      style: { bottom: 0, left: 0 },
      borders: "border-b border-l",
    },
    {
      key: "br",
      style: { bottom: 0, right: 0 },
      borders: "border-b border-r",
    },
  ] as const

  return (
    <>
      {positions.map((position) => (
        <span
          key={position.key}
          aria-hidden="true"
          className={cn(
            "pointer-events-none absolute border-solid",
            position.borders
          )}
          style={{
            width: markerSize,
            height: markerSize,
            borderColor: color,
            ...position.style,
          }}
        />
      ))}
    </>
  )
}

function CardHeader({ className, ...props }: React.ComponentProps<"div">) {
  return (
    <div
      data-slot="card-header"
      className={cn(
        "relative flex items-center justify-between gap-3 border-b border-divider px-(--card-spacing) py-(--card-spacing)",
        className
      )}
      {...props}
    />
  )
}

function CardTitle({ className, ...props }: React.ComponentProps<"div">) {
  return (
    <div
      data-slot="card-title"
      className={cn("text-sm font-semibold text-foreground", className)}
      {...props}
    />
  )
}

function CardDescription({ className, ...props }: React.ComponentProps<"div">) {
  return (
    <div
      data-slot="card-description"
      className={cn("text-sm text-muted-foreground", className)}
      {...props}
    />
  )
}

function CardAction({ className, ...props }: React.ComponentProps<"div">) {
  return (
    <div
      data-slot="card-action"
      className={cn("shrink-0 self-start", className)}
      {...props}
    />
  )
}

function CardContent({ className, ...props }: React.ComponentProps<"div">) {
  return (
    <div
      data-slot="card-content"
      className={cn("px-(--card-spacing)", className)}
      {...props}
    />
  )
}

function CardFooter({ className, ...props }: React.ComponentProps<"div">) {
  return (
    <div
      data-slot="card-footer"
      className={cn(
        "flex items-center gap-3 border-t border-divider px-(--card-spacing) py-(--card-spacing)",
        className
      )}
      {...props}
    />
  )
}

export {
  Card,
  CardHeader,
  CardFooter,
  CardTitle,
  CardAction,
  CardDescription,
  CardContent,
  CornerMarkers,
}