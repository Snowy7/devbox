import { Link } from "@tanstack/react-router"
import {
  Activity,
  Bookmark,
  Clock,
  Code2,
  Database,
  File,
  Files,
  FileText,
  Flame,
  Folder,
  FolderOpen,
  GitCommit,
  HardDrive,
  Laptop,
  ShieldCheck,
  Star,
  UserCog,
} from "lucide-react"
import type { ComponentType } from "react"

import { Button } from "@workspace/ui/components/button"
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@workspace/ui/components/table"

import {
  EmptyState,
  IconTile,
  InsetDivider,
  PanelHeader,
  TableHeadLabel,
} from "@/components/ui-primitives"
import type {
  FolderActivity,
  MachineSummary,
  SharedFolderEntry,
  SharedFolderSummary,
} from "@/lib/dashboard-api"
import {
  hydrationIcon,
  trustIcon,
  trustIconClass,
  visibilityIcon,
} from "@/lib/ui-icons"

export function FolderTable({ folders }: { folders: SharedFolderSummary[] }) {
  if (folders.length === 0) {
    return (
      <EmptyState
        icon={FolderOpen}
        message="No shared folders yet. Add a folder from the desktop app to get started."
      />
    )
  }

  return (
    <Table>
      <TableHeader>
        <TableRow className="border-0 hover:bg-transparent">
          <TableHead className="w-[45%]">
            <TableHeadLabel icon={FolderOpen}>Name</TableHeadLabel>
          </TableHead>
          <TableHead className="hidden md:table-cell">
            <TableHeadLabel icon={Clock}>Updated</TableHeadLabel>
          </TableHead>
          <TableHead className="hidden lg:table-cell">
            <TableHeadLabel icon={UserCog}>Visibility</TableHeadLabel>
          </TableHead>
          <TableHead>
            <TableHeadLabel icon={Files}>Files</TableHeadLabel>
          </TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        {folders.map((folder) => {
          const VisibilityIcon = visibilityIcon(folder.visibility)

          return (
            <TableRow key={folder.id}>
              <TableCell className="whitespace-normal">
                <Link
                  to="/folders/$folderId"
                  params={{ folderId: folder.id }}
                  search={{ tab: undefined, file: undefined }}
                  className="group block min-w-0"
                >
                  <div className="flex items-center gap-2">
                    <IconTile
                      icon={FolderOpen}
                      size="sm"
                      iconClassName="group-hover:text-signal"
                    />
                    <div className="min-w-0">
                      <p className="truncate font-medium text-foreground group-hover:text-signal">
                        {folder.displayName}
                      </p>
                      <p className="truncate text-xs text-muted-foreground">
                        {folder.description}
                      </p>
                    </div>
                  </div>
                </Link>
              </TableCell>
              <TableCell className="hidden whitespace-nowrap text-muted-foreground md:table-cell">
                <span className="inline-flex items-center gap-1.5">
                  <Clock className="size-3 text-faint" />
                  {folder.updatedAt}
                </span>
              </TableCell>
              <TableCell className="hidden whitespace-nowrap text-muted-foreground lg:table-cell">
                <span className="inline-flex items-center gap-1.5 capitalize">
                  <VisibilityIcon className="size-3 text-faint" />
                  {folder.visibility}
                </span>
              </TableCell>
              <TableCell className="whitespace-nowrap tabular-nums text-muted-foreground">
                {folder.fileCount.toLocaleString()}
              </TableCell>
            </TableRow>
          )
        })}
      </TableBody>
    </Table>
  )
}

export function MachineTable({ machines }: { machines: MachineSummary[] }) {
  if (machines.length === 0) {
    return (
      <EmptyState
        icon={Laptop}
        message="No machines registered yet. Sign in from the desktop app to trust this device."
      />
    )
  }

  return (
    <Table>
      <TableHeader>
        <TableRow className="border-0 hover:bg-transparent">
          <TableHead className="w-[55%]">
            <TableHeadLabel icon={Laptop}>Name</TableHeadLabel>
          </TableHead>
          <TableHead className="hidden md:table-cell">
            <TableHeadLabel icon={Clock}>Last seen</TableHeadLabel>
          </TableHead>
          <TableHead>
            <TableHeadLabel icon={ShieldCheck}>Trust</TableHeadLabel>
          </TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        {machines.map((machine) => {
          const TrustIcon = trustIcon(machine.trustState)

          return (
            <TableRow key={machine.id}>
              <TableCell className="whitespace-normal">
                <div className="flex items-center gap-2">
                  <IconTile icon={Laptop} size="sm" />
                  <span className="font-medium">{machine.displayName}</span>
                </div>
              </TableCell>
              <TableCell className="hidden whitespace-nowrap text-muted-foreground md:table-cell">
                <span className="inline-flex items-center gap-1.5">
                  <Clock className="size-3 text-faint" />
                  {machine.lastSeen}
                </span>
              </TableCell>
              <TableCell className="whitespace-nowrap text-muted-foreground">
                <span className="inline-flex items-center gap-1.5 capitalize">
                  <TrustIcon
                    className={`size-3.5 shrink-0 ${trustIconClass(machine.trustState)}`}
                  />
                  {machine.trustState}
                </span>
              </TableCell>
            </TableRow>
          )
        })}
      </TableBody>
    </Table>
  )
}

export function FolderDetailHeader({
  folder,
}: {
  folder: SharedFolderSummary
}) {
  const VisibilityIcon = visibilityIcon(folder.visibility)
  const HydrationIcon = hydrationIcon(folder.hydrationState)

  return (
    <div className="relative overflow-hidden border border-divider bg-card">
      <div className="flex flex-col gap-4 px-5 py-4 sm:flex-row sm:items-start sm:justify-between">
        <div className="min-w-0 space-y-3">
          <p className="text-label text-faint">/// Folder</p>
          <div className="flex items-center gap-3">
            <IconTile icon={FolderOpen} size="lg" />
            <h1 className="truncate text-xl font-semibold tracking-tight">
              {folder.displayName}
            </h1>
          </div>
          <p className="max-w-3xl text-sm text-muted-foreground">
            {folder.description}
          </p>
          <div className="flex flex-wrap gap-3 text-xs text-muted-foreground">
            <MetaChip icon={VisibilityIcon} label={folder.visibility} />
            <MetaChip icon={UserCog} label={folder.role} />
            <MetaChip icon={HydrationIcon} label={folder.hydrationState} />
            <MetaChip icon={Bookmark} label={folder.lastCheckpoint} />
          </div>
        </div>
        <div className="flex shrink-0 gap-2">
          <Button variant="outline" size="sm">
            <Star />
            Pin
          </Button>
          <Button size="sm">
            <Flame />
            Warm cache
          </Button>
        </div>
      </div>
    </div>
  )
}

export function FolderFileBrowser({
  folder,
}: {
  folder: SharedFolderSummary
}) {
  return (
    <>
      <PanelHeader
        icon={Files}
        title="Files"
        description={`/${folder.displayName}`}
      />
      <Table>
        <TableHeader>
          <TableRow className="hover:bg-transparent">
            <TableHead>
              <TableHeadLabel icon={FileText}>Name</TableHeadLabel>
            </TableHead>
            <TableHead className="hidden md:table-cell">
              <TableHeadLabel icon={Database}>Hydration</TableHeadLabel>
            </TableHead>
            <TableHead>
              <TableHeadLabel icon={HardDrive}>Size</TableHeadLabel>
            </TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {folder.entries.map((entry) => (
            <FileRow key={entry.path} entry={entry} />
          ))}
        </TableBody>
      </Table>
    </>
  )
}

export function FolderReadme({ folder }: { folder: SharedFolderSummary }) {
  return (
    <>
      <div className="relative flex items-center gap-3 px-5 py-4">
        <IconTile icon={FileText} size="sm" />
        <p className="text-lg font-semibold tracking-tight">README</p>
        <InsetDivider />
      </div>
      <div className="space-y-3 px-5 py-5">
        <h2 className="text-base font-semibold">{folder.displayName}</h2>
        <p className="text-sm leading-7 text-muted-foreground">
          {folder.readme}
        </p>
      </div>
    </>
  )
}

export function ActivityList({ activity }: { activity: FolderActivity[] }) {
  if (activity.length === 0) {
    return (
      <>
        <PanelHeader icon={Activity} title="Recent activity" />
        <EmptyState
          icon={Activity}
          message="No recent activity in this folder yet."
        />
      </>
    )
  }

  return (
    <>
      <PanelHeader icon={Activity} title="Recent activity" />
      <ul>
        {activity.map((item) => (
          <li
            key={item.id}
            className="relative flex gap-3 px-5 py-3.5 transition-colors hover:bg-muted/30"
          >
            <IconTile icon={activityIcon(item)} size="sm" />
            <div className="min-w-0 flex-1">
              <p className="text-sm font-medium">{item.title}</p>
              <p className="mt-1 text-sm text-muted-foreground">
                {item.detail}
              </p>
              <p className="mt-2 inline-flex items-center gap-1.5 text-xs text-faint">
                <Clock className="size-3" />
                {item.timestamp}
              </p>
            </div>
            <InsetDivider />
          </li>
        ))}
      </ul>
    </>
  )
}

function FileRow({ entry }: { entry: SharedFolderEntry }) {
  const Icon =
    entry.kind === "directory" ? Folder : entry.language ? Code2 : File
  const HydrationIcon = hydrationIcon(entry.hydrationState)

  return (
    <TableRow>
      <TableCell>
        <div className="flex min-w-0 items-center gap-2">
          <IconTile icon={Icon} size="sm" />
          <div className="min-w-0">
            <p className="truncate font-medium">{entry.name}</p>
            <p className="truncate text-xs text-muted-foreground">
              {entry.summary}
            </p>
          </div>
        </div>
      </TableCell>
      <TableCell className="hidden text-muted-foreground md:table-cell">
        <span className="inline-flex items-center gap-1.5">
          <HydrationIcon className="size-3 text-faint" />
          {entry.hydrationState}
        </span>
      </TableCell>
      <TableCell className="text-muted-foreground">
        <p>{entry.sizeLabel}</p>
        <p className="inline-flex items-center gap-1 text-xs text-faint">
          <Clock className="size-3" />
          {entry.updatedAt}
        </p>
      </TableCell>
    </TableRow>
  )
}

function MetaChip({
  icon: Icon,
  label,
}: {
  icon: ComponentType<{ className?: string }>
  label: string
}) {
  return (
    <span className="inline-flex items-center gap-1.5 border border-divider bg-input/50 px-2 py-1 capitalize">
      <Icon className="size-3 text-faint" />
      {label}
    </span>
  )
}

function activityIcon(item: FolderActivity) {
  const text = `${item.title} ${item.detail}`.toLowerCase()
  if (text.includes("checkpoint") || text.includes("pin")) return Bookmark
  if (text.includes("sync") || text.includes("machine")) return Laptop
  if (text.includes("file") || text.includes("edit")) return GitCommit
  return Activity
}
