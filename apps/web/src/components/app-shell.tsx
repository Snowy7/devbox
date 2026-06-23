import { Link, useRouterState } from "@tanstack/react-router"
import {
  BookOpen,
  ChevronDown,
  ExternalLink,
  FolderOpen,
  Home,
  Laptop,
  List,
  Settings,
  User,
} from "lucide-react"
import { useState, type ReactNode } from "react"

import { Button } from "@workspace/ui/components/button"
import { CornerMarkers } from "@workspace/ui/components/card"
import { cn } from "@workspace/ui/lib/utils"

import { SettingsModal } from "@/components/settings-modal"
import {
  InsetDivider,
  PageHeader,
  SearchField,
  SectionLabel,
} from "@/components/ui-primitives"
import type { DashboardData } from "@/lib/dashboard-api"
import { docsUrl } from "@/lib/public-config"

type AppShellProps = {
  data: DashboardData
  children: ReactNode
  title?: string
}

const navItems = [
  { to: "/", label: "Home", icon: Home, id: "home" },
  { to: "/folders", label: "Folders", icon: FolderOpen, id: "folders" },
  { to: "/machines", label: "Machines", icon: Laptop, id: "machines" },
] as const

const pageIcons = {
  home: Home,
  folders: FolderOpen,
  machines: Laptop,
} as const

export function AppShell({ data, children, title }: AppShellProps) {
  const pathname = useRouterState({
    select: (state) => state.location.pathname,
  })
  const active = activeNavItem(pathname)
  const pageTitle = title ?? pageTitleFromPath(pathname)
  const [settingsOpen, setSettingsOpen] = useState(false)
  const [folderFilter, setFolderFilter] = useState("")

  const initials = data.identity.account.displayName
    .split(/\s+/)
    .map((part) => part[0])
    .join("")
    .slice(0, 2)
    .toUpperCase()

  const filteredFolders = data.folders.filter((folder) => {
    const query = folderFilter.trim().toLowerCase()
    if (!query) return true
    return (
      folder.displayName.toLowerCase().includes(query) ||
      folder.description.toLowerCase().includes(query)
    )
  })

  return (
    <div className="flex h-svh flex-col overflow-hidden bg-background text-foreground">
      <div className="flex min-h-0 flex-1 shell-gap shell-padding">
        <aside
          className="relative hidden shrink-0 flex-col bg-card lg:flex"
          style={{ width: "var(--token-sidebar-width)" }}
        >
          <CornerMarkers />

          <div className="relative px-3 pt-6 pb-3">
            <InsetDivider side="bottom" />
            <Link to="/" className="flex items-center gap-3">
              <span className="grid size-7 place-items-center border border-divider bg-input">
                <FolderOpen className="size-4 text-signal" />
              </span>
              <div className="min-w-0">
                <p className="text-label text-foreground">Bindhub</p>
                <p className="text-label text-muted-foreground">Workspace</p>
              </div>
            </Link>
          </div>

          <SectionLabel>/// Console</SectionLabel>
          <nav className="flex flex-col">
            {navItems.map((item) => {
              const Icon = item.icon
              const isActive = active === item.id

              return (
                <Link
                  key={item.id}
                  to={item.to}
                  className={cn(
                    "relative flex w-full items-center gap-2 px-4 py-2.5 text-sm transition-colors",
                    isActive
                      ? "font-medium text-signal"
                      : "text-muted-foreground hover:bg-muted hover:text-foreground"
                  )}
                >
                  <Icon className="size-4 shrink-0" />
                  <span className="min-w-0 flex-1 truncate">{item.label}</span>
                  {isActive ? (
                    <span
                      aria-hidden
                      className="size-1.5 shrink-0 bg-signal shadow-[0_0_6px_var(--signal)]"
                    />
                  ) : null}
                </Link>
              )
            })}
          </nav>

          <div className="relative mt-4 flex min-h-0 flex-1 flex-col pt-3">
            <InsetDivider side="top" />
            <div className="flex items-center justify-between gap-2 px-3 pb-2 pt-3">
              <p className="text-label">/// Folders</p>
              <Link
                to="/folders"
                className="inline-flex items-center gap-1 text-xs text-signal hover:underline"
              >
                <List className="size-3" />
                All
              </Link>
            </div>
            <div className="px-3 pb-2">
              <SearchField
                placeholder="Find a folder"
                className="h-7 max-w-none text-xs"
                value={folderFilter}
                onChange={(event) => setFolderFilter(event.target.value)}
              />
            </div>
            <div className="min-h-0 flex-1 overflow-y-auto">
              {filteredFolders.slice(0, 10).map((folder) => (
                <Link
                  key={folder.id}
                  to="/folders/$folderId"
                  params={{ folderId: folder.id }}
                  search={{ tab: undefined, file: undefined }}
                  className="flex items-center gap-2 px-3 py-2 transition-colors hover:bg-muted"
                >
                  <span className="grid size-6 shrink-0 place-items-center border border-divider bg-input">
                    <FolderOpen className="size-3 text-muted-foreground" />
                  </span>
                  <div className="min-w-0">
                    <p className="truncate text-xs font-medium text-foreground">
                      {folder.displayName}
                    </p>
                    <p className="truncate text-[10px] uppercase tracking-wider text-faint">
                      {folder.fileCount} files · {folder.visibility}
                    </p>
                  </div>
                </Link>
              ))}
            </div>
          </div>

          <div className="relative shrink-0">
            <InsetDivider side="top" />
            <button
              type="button"
              onClick={() => setSettingsOpen(true)}
              className="flex w-full items-center gap-3 px-3 py-3 text-left transition-colors hover:bg-muted"
            >
              <span className="grid size-8 shrink-0 place-items-center border border-divider bg-input text-xs font-semibold">
                {initials || "D"}
              </span>
              <div className="min-w-0 flex-1">
                <p className="truncate text-sm font-medium text-foreground">
                  {data.identity.account.displayName}
                </p>
                <p className="truncate text-xs text-muted-foreground">
                  {data.identity.account.email}
                </p>
              </div>
              <ChevronDown className="size-3.5 shrink-0 text-muted-foreground" />
            </button>
            <div className="flex flex-col px-1 pb-2">
              <a
                href={docsUrl}
                className="flex items-center gap-2 px-3 py-2 text-sm text-muted-foreground transition-colors hover:bg-muted hover:text-foreground"
              >
                <BookOpen className="size-4" />
                Docs
                <ExternalLink className="ml-auto size-3 text-faint" />
              </a>
              <button
                type="button"
                onClick={() => setSettingsOpen(true)}
                className="flex w-full items-center gap-2 px-3 py-2 text-left text-sm text-muted-foreground transition-colors hover:bg-muted hover:text-foreground"
              >
                <Settings className="size-4" />
                Settings
              </button>
            </div>
          </div>
        </aside>

        <div className="flex min-w-0 flex-1 flex-col overflow-hidden">
          <header
            className="relative flex shrink-0 items-center gap-3 bg-card px-4 lg:hidden"
            style={{ height: "var(--token-header-height)" }}
          >
            <CornerMarkers />
            <Link to="/" className="shrink-0 font-semibold text-foreground">
              Bindhub
            </Link>
            <nav className="flex min-w-0 flex-1 gap-1 overflow-x-auto">
              {navItems.map((item) => {
                const Icon = item.icon
                return (
                  <Link
                    key={item.id}
                    to={item.to}
                    className={cn(
                      "inline-flex shrink-0 items-center gap-1 px-2.5 py-1.5 text-sm transition-colors",
                      active === item.id
                        ? "font-medium text-signal"
                        : "text-muted-foreground hover:text-foreground"
                    )}
                  >
                    <Icon className="size-3.5" />
                    {item.label}
                  </Link>
                )
              })}
            </nav>
            <button
              type="button"
              onClick={() => setSettingsOpen(true)}
              className="grid size-8 shrink-0 place-items-center border border-divider bg-input text-xs font-semibold"
              aria-label="Open settings"
            >
              {initials || "D"}
            </button>
            <InsetDivider />
          </header>

          <div className="hidden lg:block">
            <PageHeader
              title={pageTitle}
              icon={pageIcons[active]}
              search={
                <SearchField
                  placeholder="Find a folder or machine"
                  className="w-full max-w-md"
                />
              }
              action={
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  className="gap-1.5"
                  onClick={() => setSettingsOpen(true)}
                >
                  <User className="size-3.5 text-muted-foreground" />
                  {data.identity.account.displayName}
                  <ChevronDown className="size-3.5" />
                </Button>
              }
            />
          </div>

          <main className="min-h-0 flex-1 overflow-y-auto pr-1">{children}</main>
        </div>
      </div>

      <SettingsModal
        open={settingsOpen}
        onOpenChange={setSettingsOpen}
        data={data}
      />
    </div>
  )
}

function activeNavItem(pathname: string): (typeof navItems)[number]["id"] {
  if (pathname.startsWith("/folders")) return "folders"
  if (pathname.startsWith("/machines")) return "machines"
  return "home"
}

function pageTitleFromPath(pathname: string) {
  if (pathname.startsWith("/folders")) return "Folders"
  if (pathname.startsWith("/machines")) return "Machines"
  return "Home"
}
