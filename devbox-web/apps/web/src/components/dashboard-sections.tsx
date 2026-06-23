import { Badge } from "@workspace/ui/components/badge"

import type {
  DashboardData,
  MachineSummary,
  SharedFolderSummary,
} from "@/lib/dashboard-api"

export function OverviewSection({ data }: { data: DashboardData }) {
  return (
    <div className="grid gap-3 sm:grid-cols-3">
      <Metric label="Folders" value={data.overview.folderCount} />
      <Metric label="Machines" value={data.overview.machineCount} />
      <Metric label="Trusted" value={data.overview.trustedMachineCount} />
    </div>
  )
}

export function FolderList({ folders }: { folders: SharedFolderSummary[] }) {
  return (
    <div className="overflow-hidden rounded-lg border">
      {folders.map((folder) => (
        <div
          key={folder.id}
          className="grid gap-3 border-b p-4 last:border-b-0 sm:grid-cols-[1fr_auto]"
        >
          <div className="min-w-0">
            <h2 className="truncate font-medium">{folder.displayName}</h2>
            <p className="mt-1 text-sm text-muted-foreground">
              {folder.lastCheckpoint}
            </p>
          </div>
          <div className="flex flex-wrap items-center gap-2">
            <Badge variant="secondary">{folder.role}</Badge>
            <Badge variant="outline">{folder.hydrationState}</Badge>
          </div>
        </div>
      ))}
    </div>
  )
}

export function MachineList({ machines }: { machines: MachineSummary[] }) {
  return (
    <div className="overflow-hidden rounded-lg border">
      {machines.map((machine) => (
        <div
          key={machine.id}
          className="grid gap-3 border-b p-4 last:border-b-0 sm:grid-cols-[1fr_auto]"
        >
          <div className="min-w-0">
            <h2 className="truncate font-medium">{machine.displayName}</h2>
            <p className="mt-1 text-sm text-muted-foreground">
              {machine.lastSeen}
            </p>
          </div>
          <Badge
            variant={machine.trustState === "trusted" ? "secondary" : "outline"}
          >
            {machine.trustState}
          </Badge>
        </div>
      ))}
    </div>
  )
}

export function DataSourceNote({ data }: { data: DashboardData }) {
  return (
    <p className="text-sm text-muted-foreground">
      Data source: {data.source.label}
    </p>
  )
}

function Metric({ label, value }: { label: string; value: number }) {
  return (
    <div className="rounded-lg border p-4">
      <p className="text-sm text-muted-foreground">{label}</p>
      <p className="mt-2 text-2xl font-semibold tracking-normal">{value}</p>
    </div>
  )
}
