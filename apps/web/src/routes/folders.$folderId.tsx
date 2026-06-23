import { Link, createFileRoute, notFound } from "@tanstack/react-router"
import {
  Activity,
  Archive,
  Bookmark,
  Boxes,
  CheckCircle2,
  ChevronRight,
  CirclePause,
  Clock,
  Code2,
  Database,
  File,
  FileText,
  Folder,
  FolderOpen,
  GitBranch,
  HardDrive,
  History,
  Laptop,
  LockKeyhole,
  MoreHorizontal,
  RefreshCcw,
  Settings,
  ShieldCheck,
  SlidersHorizontal,
  Sparkles,
  Users,
} from "lucide-react"
import type { LucideIcon } from "lucide-react"

import { Badge } from "@workspace/ui/components/badge"
import { Button } from "@workspace/ui/components/button"
import { Switch } from "@workspace/ui/components/switch"
import { cn } from "@workspace/ui/lib/utils"

import { AppShell } from "@/components/app-shell"
import {
  EmptyState,
  IconTile,
  InsetDivider,
  Panel,
  PanelHeader,
} from "@/components/ui-primitives"
import { loadAuthenticatedDashboard } from "@/lib/dashboard-loader"
import type {
  FolderRevision,
  SharedFolderEntry,
  SharedFolderSummary,
} from "@/lib/dashboard-api"
import { hydrationIcon, visibilityIcon } from "@/lib/ui-icons"

type FolderTab = "browse" | "activity" | "revisions" | "settings"

export const Route = createFileRoute("/folders/$folderId")({
  validateSearch: (search: Record<string, unknown>) => ({
    file: typeof search.file === "string" ? search.file : undefined,
    tab: isFolderTab(search.tab) ? search.tab : undefined,
  }),
  loader: async ({ location }) => loadAuthenticatedDashboard(location.pathname),
  component: FolderDetailPage,
})

function FolderDetailPage() {
  const { folderId } = Route.useParams()
  const search = Route.useSearch()
  const data = Route.useLoaderData()
  const folder = data.folders.find((item) => item.id === folderId)

  if (!folder) {
    throw notFound()
  }

  const tab = search.tab ?? "browse"
  const selectedEntry =
    folder.entries.find((entry) => entry.path === search.file) ??
    folder.entries.find((entry) => entry.path === "README.md") ??
    folder.entries[0]

  return (
    <AppShell data={data} title={folder.displayName}>
      <div className="shell-gap flex flex-col pt-4">
        <FolderHero folder={folder} />
        <FolderTabs folder={folder} activeTab={tab} selectedFile={selectedEntry?.path} />

        {tab === "browse" ? (
          <BrowseTab folder={folder} selectedEntry={selectedEntry} />
        ) : tab === "activity" ? (
          <ActivityTab folder={folder} />
        ) : tab === "revisions" ? (
          <RevisionsTab folder={folder} />
        ) : (
          <SettingsTab folder={folder} />
        )}
      </div>
    </AppShell>
  )
}

function FolderHero({ folder }: { folder: SharedFolderSummary }) {
  const VisibilityIcon = visibilityIcon(folder.visibility)
  const HydrationIcon = hydrationIcon(folder.hydrationState)
  const SyncIcon = folder.syncStatus === "synced" ? CheckCircle2 : RefreshCcw

  return (
    <Panel markers={false}>
      <div className="relative px-5 py-5">
        <div className="flex flex-col gap-5 lg:flex-row lg:items-start lg:justify-between">
          <div className="min-w-0 space-y-4">
            <div className="flex items-center gap-3">
              <IconTile icon={FolderOpen} size="lg" iconClassName="text-signal" />
              <div className="min-w-0">
                <p className="text-label text-faint">/// Shared folder</p>
                <h1 className="truncate text-2xl font-semibold tracking-tight">
                  {folder.displayName}
                </h1>
              </div>
            </div>
            <p className="max-w-4xl text-sm leading-6 text-muted-foreground">
              {folder.description}
            </p>
            <div className="flex flex-wrap gap-2">
              <MetaBadge icon={SyncIcon} label={folder.syncStatus} />
              <MetaBadge icon={VisibilityIcon} label={folder.visibility} />
              <MetaBadge icon={HydrationIcon} label={folder.hydrationState} />
              <MetaBadge icon={HardDrive} label={folder.sizeLabel} />
              <MetaBadge icon={Laptop} label={`${folder.machineCount} machines`} />
            </div>
          </div>
          <div className="grid gap-2 sm:grid-cols-3 lg:min-w-[360px]">
            <Metric label="Files" value={folder.fileCount.toLocaleString()} icon={FileText} />
            <Metric label="Updated" value={folder.updatedAt} icon={Clock} />
            <Metric label="Path" value={folder.localPath} icon={Folder} />
          </div>
        </div>
        <InsetDivider />
      </div>
    </Panel>
  )
}

function FolderTabs({
  folder,
  activeTab,
  selectedFile,
}: {
  folder: SharedFolderSummary
  activeTab: FolderTab
  selectedFile?: string
}) {
  const tabs: Array<{ value: FolderTab; label: string; icon: LucideIcon }> = [
    { value: "browse", label: "Browse", icon: FolderOpen },
    { value: "activity", label: "Activity", icon: Activity },
    { value: "revisions", label: "Revisions", icon: History },
    { value: "settings", label: "Settings", icon: Settings },
  ]

  return (
    <nav className="flex flex-wrap gap-2">
      {tabs.map((tab) => {
        const Icon = tab.icon
        const isActive = tab.value === activeTab

        return (
          <Button
            key={tab.value}
            asChild
            variant={isActive ? "default" : "outline"}
            size="sm"
            className="rounded-md"
          >
            <Link
              to="/folders/$folderId"
              params={{ folderId: folder.id }}
              search={{
                tab: tab.value,
                file: selectedFile && tab.value === "browse" ? selectedFile : undefined,
              }}
            >
              <Icon />
              {tab.label}
            </Link>
          </Button>
        )
      })}
    </nav>
  )
}

function BrowseTab({
  folder,
  selectedEntry,
}: {
  folder: SharedFolderSummary
  selectedEntry?: SharedFolderEntry
}) {
  return (
    <div className="shell-gap grid grid-cols-1 xl:grid-cols-[320px_minmax(0,1fr)]">
      <Panel markers={false}>
        <PanelHeader
          icon={Folder}
          title="Folder tree"
          description={folder.localPath}
        />
        <FileTree folder={folder} selectedPath={selectedEntry?.path} />
      </Panel>

      <div className="shell-gap flex min-w-0 flex-col">
        <Panel markers={false}>
          {selectedEntry ? (
            <EntryPreview folder={folder} entry={selectedEntry} />
          ) : (
            <EmptyState icon={File} message="No files are available yet." />
          )}
        </Panel>
        <Panel markers={false}>
          <ChildrenTable folder={folder} parentPath={selectedEntry?.kind === "directory" ? selectedEntry.path : selectedEntry?.parentPath ?? null} />
        </Panel>
      </div>
    </div>
  )
}

function FileTree({
  folder,
  selectedPath,
}: {
  folder: SharedFolderSummary
  selectedPath?: string
}) {
  const rootEntries = folder.entries.filter((entry) => entry.parentPath === null)

  return (
    <div className="max-h-[620px] overflow-y-auto px-2 py-2">
      <TreeEntries folder={folder} entries={rootEntries} selectedPath={selectedPath} depth={0} />
    </div>
  )
}

function TreeEntries({
  folder,
  entries,
  selectedPath,
  depth,
}: {
  folder: SharedFolderSummary
  entries: SharedFolderEntry[]
  selectedPath?: string
  depth: number
}) {
  const sorted = [...entries].sort((a, b) => {
    if (a.kind !== b.kind) return a.kind === "directory" ? -1 : 1
    return a.name.localeCompare(b.name)
  })

  return (
    <ul className="space-y-1">
      {sorted.map((entry) => {
        const children = folder.entries.filter(
          (candidate) => candidate.parentPath === entry.path
        )

        return (
          <li key={entry.path}>
            <FileTreeLink
              folder={folder}
              entry={entry}
              selected={selectedPath === entry.path}
              depth={depth}
            />
            {children.length > 0 ? (
              <TreeEntries
                folder={folder}
                entries={children}
                selectedPath={selectedPath}
                depth={depth + 1}
              />
            ) : null}
          </li>
        )
      })}
    </ul>
  )
}

function FileTreeLink({
  folder,
  entry,
  selected,
  depth,
}: {
  folder: SharedFolderSummary
  entry: SharedFolderEntry
  selected: boolean
  depth: number
}) {
  const EntryIcon = entry.kind === "directory" ? Folder : fileIcon(entry)

  return (
    <Link
      to="/folders/$folderId"
      params={{ folderId: folder.id }}
      search={{ tab: "browse", file: entry.path }}
      className={cn(
        "flex items-center gap-2 rounded-md px-2 py-1.5 text-sm transition-colors",
        selected
          ? "bg-muted font-medium text-foreground"
          : "text-muted-foreground hover:bg-muted/60 hover:text-foreground"
      )}
      style={{ paddingLeft: `${0.5 + depth * 1.1}rem` }}
    >
      <EntryIcon className="size-4 shrink-0 text-faint" />
      <span className="min-w-0 flex-1 truncate">{entry.name}</span>
      {entry.hydrationState === "remote-only" ? (
        <Archive className="size-3 shrink-0 text-faint" />
      ) : null}
    </Link>
  )
}

function EntryPreview({
  folder,
  entry,
}: {
  folder: SharedFolderSummary
  entry: SharedFolderEntry
}) {
  const EntryIcon = entry.kind === "directory" ? FolderOpen : fileIcon(entry)
  const HydrationIcon = hydrationIcon(entry.hydrationState)

  return (
    <>
      <PanelHeader
        icon={EntryIcon}
        title={entry.name}
        description={entry.path}
        action={
          <Button size="sm" variant="outline" className="rounded-md">
            <MoreHorizontal />
            Actions
          </Button>
        }
      />
      <div className="space-y-5 px-5 py-5">
        <div className="grid gap-3 md:grid-cols-3">
          <InfoCell icon={HydrationIcon} label="Hydration" value={entry.hydrationState} />
          <InfoCell icon={HardDrive} label="Size" value={entry.sizeLabel} />
          <InfoCell icon={Clock} label="Updated" value={entry.updatedAt} />
        </div>

        {entry.kind === "directory" ? (
          <div className="rounded-lg border bg-muted/30 px-4 py-4">
            <p className="text-sm font-medium">Directory</p>
            <p className="mt-1 text-sm leading-6 text-muted-foreground">
              {entry.summary}
            </p>
          </div>
        ) : (
          <div className="overflow-hidden rounded-lg border bg-muted/20">
            <div className="flex items-center justify-between border-b px-4 py-2">
              <span className="text-xs font-medium uppercase tracking-wider text-muted-foreground">
                {entry.language ?? "Text"}
              </span>
              <Badge variant="outline">{folder.lastCheckpoint}</Badge>
            </div>
            <pre className="max-h-[420px] overflow-auto p-4 text-sm leading-6">
              <code>{entry.content ?? entry.summary}</code>
            </pre>
          </div>
        )}
      </div>
    </>
  )
}

function ChildrenTable({
  folder,
  parentPath,
}: {
  folder: SharedFolderSummary
  parentPath: string | null
}) {
  const children = folder.entries.filter((entry) => entry.parentPath === parentPath)

  return (
    <>
      <PanelHeader
        icon={Boxes}
        title="Contents"
        description={parentPath ? `/${parentPath}` : `/${folder.displayName}`}
      />
      {children.length === 0 ? (
        <EmptyState icon={File} message="No child entries at this path." />
      ) : (
        <div className="divide-y">
          {children.map((entry) => (
            <Link
              key={entry.path}
              to="/folders/$folderId"
              params={{ folderId: folder.id }}
              search={{ tab: "browse", file: entry.path }}
              className="flex items-center gap-3 px-5 py-3 transition-colors hover:bg-muted/50"
            >
              <IconTile icon={entry.kind === "directory" ? Folder : fileIcon(entry)} size="sm" />
              <div className="min-w-0 flex-1">
                <p className="truncate text-sm font-medium">{entry.name}</p>
                <p className="truncate text-xs text-muted-foreground">{entry.summary}</p>
              </div>
              <span className="hidden text-xs text-muted-foreground md:block">
                {entry.sizeLabel}
              </span>
              <ChevronRight className="size-4 text-faint" />
            </Link>
          ))}
        </div>
      )}
    </>
  )
}

function ActivityTab({ folder }: { folder: SharedFolderSummary }) {
  return (
    <div className="shell-gap grid grid-cols-1 xl:grid-cols-[minmax(0,1fr)_320px]">
      <Panel markers={false}>
        <PanelHeader icon={Activity} title="Activity" description="Recent folder events" />
        <div className="divide-y">
          {folder.recentActivity.map((item) => (
            <div key={item.id} className="flex gap-3 px-5 py-4">
              <IconTile icon={activityIcon(item.title)} size="sm" />
              <div className="min-w-0 flex-1">
                <p className="text-sm font-medium">{item.title}</p>
                <p className="mt-1 text-sm leading-6 text-muted-foreground">{item.detail}</p>
                <p className="mt-2 inline-flex items-center gap-1.5 text-xs text-faint">
                  <Clock className="size-3" />
                  {item.timestamp}
                </p>
              </div>
            </div>
          ))}
        </div>
      </Panel>
      <Panel markers={false}>
        <PanelHeader icon={Sparkles} title="Status" description="Sync and cache posture" />
        <div className="space-y-3 px-5 py-5">
          <InfoCell icon={RefreshCcw} label="Sync" value={folder.syncStatus} />
          <InfoCell icon={Database} label="Cache policy" value={folder.settings.cachePolicy} />
          <InfoCell icon={ShieldCheck} label="Secret protection" value={folder.settings.protectSecrets ? "Enabled" : "Off"} />
        </div>
      </Panel>
    </div>
  )
}

function RevisionsTab({ folder }: { folder: SharedFolderSummary }) {
  return (
    <Panel markers={false}>
      <PanelHeader
        icon={History}
        title="Revisions"
        description="Loom folder revisions and checkpoints"
        action={
          <Button size="sm" className="rounded-md">
            <Bookmark />
            New checkpoint
          </Button>
        }
      />
      <div className="divide-y">
        {folder.revisions.map((revision) => (
          <RevisionRow key={revision.id} revision={revision} />
        ))}
      </div>
    </Panel>
  )
}

function RevisionRow({ revision }: { revision: FolderRevision }) {
  return (
    <div className="flex flex-col gap-3 px-5 py-4 md:flex-row md:items-center">
      <IconTile icon={revision.kind === "checkpoint" ? Bookmark : GitBranch} size="sm" />
      <div className="min-w-0 flex-1">
        <div className="flex flex-wrap items-center gap-2">
          <p className="font-medium">{revision.label}</p>
          <Badge variant={revision.kind === "checkpoint" ? "default" : "secondary"}>
            {revision.kind}
          </Badge>
          {revision.pinned ? <Badge variant="outline">Pinned</Badge> : null}
        </div>
        <p className="mt-1 text-sm text-muted-foreground">{revision.message}</p>
      </div>
      <div className="shrink-0 text-sm text-muted-foreground md:text-right">
        <p>{revision.createdAt}</p>
        <p className="text-xs text-faint">
          {revision.changedFiles} files by {revision.author}
        </p>
      </div>
    </div>
  )
}

function SettingsTab({ folder }: { folder: SharedFolderSummary }) {
  return (
    <div className="shell-gap grid grid-cols-1 xl:grid-cols-[minmax(0,1fr)_360px]">
      <Panel markers={false}>
        <PanelHeader
          icon={SlidersHorizontal}
          title="Folder settings"
          description="Controls are staged for the hosted settings API."
        />
        <div className="divide-y">
          <SettingRow
            icon={RefreshCcw}
            title="Live sync"
            description="Keep this folder moving between trusted machines."
            checked={folder.settings.syncState === "live"}
          />
          <SettingRow
            icon={LockKeyhole}
            title="Secret protection"
            description="Block known secret-like files from hosted object upload."
            checked={folder.settings.protectSecrets}
          />
          <SettingRow
            icon={GitBranch}
            title="Git metadata boundary"
            description="Respect Git folders without treating Bindhub as Git."
            checked={folder.settings.includeGitMetadata}
          />
          <SettingRow
            icon={Sparkles}
            title="Agent sandboxes"
            description="Allow isolated parallel workspaces for agents."
            checked={folder.settings.allowAgentSandboxes}
          />
        </div>
      </Panel>

      <Panel markers={false}>
        <PanelHeader icon={ShieldCheck} title="Policy" description="Current folder posture" />
        <div className="space-y-3 px-5 py-5">
          <InfoCell icon={Users} label="Visibility" value={folder.settings.visibility} />
          <InfoCell icon={Database} label="Cache policy" value={folder.settings.cachePolicy} />
          <InfoCell icon={CirclePause} label="Sync state" value={folder.settings.syncState} />
          <Button variant="outline" className="mt-2 w-full rounded-md">
            Save changes
          </Button>
        </div>
      </Panel>
    </div>
  )
}

function SettingRow({
  icon,
  title,
  description,
  checked,
}: {
  icon: LucideIcon
  title: string
  description: string
  checked: boolean
}) {
  return (
    <div className="flex items-center gap-4 px-5 py-4">
      <IconTile icon={icon} size="sm" />
      <div className="min-w-0 flex-1">
        <p className="text-sm font-medium">{title}</p>
        <p className="mt-1 text-sm text-muted-foreground">{description}</p>
      </div>
      <Switch checked={checked} disabled />
    </div>
  )
}

function Metric({
  label,
  value,
  icon,
}: {
  label: string
  value: string
  icon: LucideIcon
}) {
  const Icon = icon

  return (
    <div className="relative rounded-lg border bg-muted/30 px-3 py-3">
      <div className="flex items-center gap-2 text-xs text-muted-foreground">
        <Icon className="size-3.5 text-faint" />
        {label}
      </div>
      <p className="mt-2 truncate text-sm font-medium">{value}</p>
    </div>
  )
}

function MetaBadge({ icon: Icon, label }: { icon: LucideIcon; label: string }) {
  return (
    <Badge variant="outline" className="capitalize">
      <Icon className="size-3" />
      {label}
    </Badge>
  )
}

function InfoCell({
  icon,
  label,
  value,
}: {
  icon: LucideIcon
  label: string
  value: string
}) {
  const Icon = icon

  return (
    <div className="rounded-lg border bg-background px-3 py-3">
      <p className="flex items-center gap-2 text-xs text-muted-foreground">
        <Icon className="size-3.5 text-faint" />
        {label}
      </p>
      <p className="mt-2 truncate text-sm font-medium capitalize">{value}</p>
    </div>
  )
}

function fileIcon(entry: SharedFolderEntry) {
  if (entry.language === "Markdown") return FileText
  if (entry.language) return Code2
  return File
}

function activityIcon(value: string) {
  const normalized = value.toLowerCase()
  if (normalized.includes("sync")) return RefreshCcw
  if (normalized.includes("review")) return ShieldCheck
  if (normalized.includes("move")) return FolderOpen
  return Activity
}

function isFolderTab(value: unknown): value is FolderTab {
  return (
    value === "browse" ||
    value === "activity" ||
    value === "revisions" ||
    value === "settings"
  )
}
