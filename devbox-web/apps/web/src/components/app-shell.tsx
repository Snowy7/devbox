import { Link, useRouterState } from "@tanstack/react-router"
import { FolderOpen, Laptop, LogOut, PanelLeft } from "lucide-react"
import type { ReactNode } from "react"

import { Button } from "@workspace/ui/components/button"

import { authRoutes } from "@/lib/auth"
import type { DashboardData } from "@/lib/dashboard-api"

type AppShellProps = {
  data: DashboardData
  children: ReactNode
}

const navItems = [
  { to: "/dashboard", label: "Overview", icon: PanelLeft, id: "overview" },
  {
    to: "/dashboard/folders",
    label: "Folders",
    icon: FolderOpen,
    id: "folders",
  },
  {
    to: "/dashboard/machines",
    label: "Machines",
    icon: Laptop,
    id: "machines",
  },
] as const

export function AppShell({ data, children }: AppShellProps) {
  const pathname = useRouterState({
    select: (state) => state.location.pathname,
  })
  const active = activeNavItem(pathname)

  return (
    <div className="min-h-svh bg-background">
      <header className="border-b bg-background">
        <div className="mx-auto flex max-w-6xl items-center justify-between gap-4 px-5 py-4">
          <div className="min-w-0">
            <p className="text-sm text-muted-foreground">Devbox</p>
            <h1 className="truncate text-xl font-semibold tracking-normal">
              {data.identity.account.displayName}
            </h1>
          </div>
          <Button asChild variant="outline" size="sm">
            <a href={authRoutes.signOut}>
              <LogOut />
              Sign out
            </a>
          </Button>
        </div>
      </header>
      <div className="mx-auto grid max-w-6xl gap-6 px-5 py-6 md:grid-cols-[180px_1fr]">
        <nav className="flex gap-2 md:flex-col">
          {navItems.map((item) => {
            const Icon = item.icon

            return (
              <Button
                key={item.id}
                asChild
                variant={active === item.id ? "secondary" : "ghost"}
                className="justify-start"
              >
                <Link to={item.to}>
                  <Icon />
                  {item.label}
                </Link>
              </Button>
            )
          })}
        </nav>
        <main className="min-w-0 space-y-6">{children}</main>
      </div>
    </div>
  )
}

function activeNavItem(pathname: string): (typeof navItems)[number]["id"] {
  if (pathname.startsWith("/dashboard/folders")) {
    return "folders"
  }

  if (pathname.startsWith("/dashboard/machines")) {
    return "machines"
  }

  return "overview"
}
