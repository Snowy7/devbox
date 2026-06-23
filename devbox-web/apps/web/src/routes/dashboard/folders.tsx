import { createFileRoute, getRouteApi } from "@tanstack/react-router"

import { DataSourceNote, FolderList } from "@/components/dashboard-sections"

const dashboardRoute = getRouteApi("/dashboard")

export const Route = createFileRoute("/dashboard/folders")({
  component: FoldersPage,
})

function FoldersPage() {
  const dashboardData = dashboardRoute.useLoaderData()

  return (
    <>
      <section className="space-y-2">
        <h2 className="text-2xl font-semibold tracking-normal">Folders</h2>
        <DataSourceNote data={dashboardData} />
      </section>
      <FolderList folders={dashboardData.folders} />
    </>
  )
}
