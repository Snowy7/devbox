import { createFileRoute } from "@tanstack/react-router"

import { startGoogleOAuth } from "@/lib/custom-auth"

export const Route = createFileRoute("/auth/google")({
  server: {
    handlers: {
      GET: async ({ request }: { request: Request }) => {
        return startGoogleOAuth(request)
      },
    },
  },
})
