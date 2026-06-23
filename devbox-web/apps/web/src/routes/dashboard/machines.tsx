import { createFileRoute, getRouteApi } from "@tanstack/react-router"

import { DataSourceNote, MachineList } from "@/components/dashboard-sections"

const dashboardRoute = getRouteApi("/dashboard")

export const Route = createFileRoute("/dashboard/machines")({
  component: MachinesPage,
})

function MachinesPage() {
  const dashboardData = dashboardRoute.useLoaderData()

  return (
    <>
      <section className="space-y-2">
        <h2 className="text-2xl font-semibold tracking-normal">Machines</h2>
        <DataSourceNote data={dashboardData} />
      </section>
      <MachineList machines={dashboardData.machines} />
    </>
  )
}
