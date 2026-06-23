import { createFileRoute } from "@tanstack/react-router"

import { startAuthkitInitiate } from "@/lib/custom-auth"

export const Route = createFileRoute("/auth/initiate")({
  server: {
    handlers: {
      GET: async ({ request }: { request: Request }) => {
        return startAuthkitInitiate(request)
      },
    },
  },
})
