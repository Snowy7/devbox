import { createFileRoute } from "@tanstack/react-router"
import { getSignInUrl } from "@workos/authkit-tanstack-react-start"

import { defaultSignedInPath, safeReturnPathname } from "@/lib/auth"

export const Route = createFileRoute("/auth/sign-in")({
  server: {
    handlers: {
      GET: async ({ request }: { request: Request }) => {
        const returnPathname =
          safeReturnPathname(
            new URL(request.url).searchParams.get("returnPathname")
          ) ?? defaultSignedInPath
        const url = await getSignInUrl({ data: { returnPathname } })

        return Response.redirect(url, 307)
      },
    },
  },
})
