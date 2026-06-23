import { createFileRoute, getRouteApi } from "@tanstack/react-router"

import {
  DataSourceNote,
  FolderList,
  MachineList,
  OverviewSection,
} from "@/components/dashboard-sections"

const dashboardRoute = getRouteApi("/dashboard")

export const Route = createFileRoute("/dashboard/")({
  component: DashboardOverviewPage,
})

function DashboardOverviewPage() {
  const data = dashboardRoute.useLoaderData()

  return (
    <>
      <section className="space-y-2">
        <h2 className="text-2xl font-semibold tracking-normal">
          Folders and machines
        </h2>
        <DataSourceNote data={data} />
      </section>
      <OverviewSection data={data} />
      <section className="space-y-3">
        <h3 className="text-lg font-medium">Folders</h3>
        <FolderList folders={data.folders.slice(0, 3)} />
      </section>
      <section className="space-y-3">
        <h3 className="text-lg font-medium">Machines</h3>
        <MachineList machines={data.machines.slice(0, 3)} />
      </section>
    </>
  )
}
