import { createFileRoute } from "@tanstack/react-router"
import { getAuth } from "@workos/authkit-tanstack-react-start"

import { authRoutes } from "@/lib/auth"
import {
  approveCliDeviceLogin,
  approveLocalDevCliDeviceLogin,
} from "@/lib/dashboard-api"

export const Route = createFileRoute("/auth/cli")({
  server: {
    handlers: {
      GET: async ({ request }: { request: Request }) => {
        const url = new URL(request.url)
        const code = safeUserCode(url.searchParams.get("code"))

        if (!code) {
          return htmlResponse(
            "Bindhub machine login",
            "Missing machine login code.",
            400
          )
        }

        const returnPathname = `/auth/cli?code=${encodeURIComponent(code)}`
        const auth = await getAuth()

        if (!auth.user) {
          if (process.env.BINDHUB_LOCAL_DEV_CLI_AUTH === "1") {
            await approveLocalDevCliDeviceLogin(code)

            return htmlResponse(
              "Bindhub machine connected",
              "This machine is connected. You can close this tab."
            )
          }

          return Response.redirect(
            `${authRoutes.signIn}?returnPathname=${encodeURIComponent(returnPathname)}`,
            307
          )
        }

        await approveCliDeviceLogin(code, auth)

        return htmlResponse(
          "Bindhub machine connected",
          "This machine is connected. You can close this tab."
        )
      },
    },
  },
})

function safeUserCode(value: string | null): string | undefined {
  const code = value?.trim().toUpperCase()

  if (!code || !/^[A-Z0-9][A-Z0-9-]{2,31}$/.test(code)) {
    return undefined
  }

  return code
}

function htmlResponse(title: string, message: string, status = 200): Response {
  const escapedTitle = escapeHtml(title)
  const escapedMessage = escapeHtml(message)

  return new Response(
    `<!doctype html><html><head><meta charset="utf-8"><title>${escapedTitle}</title><meta name="viewport" content="width=device-width, initial-scale=1"></head><body><main style="font-family: system-ui, sans-serif; max-width: 40rem; margin: 12vh auto; padding: 0 1.5rem;"><h1>${escapedTitle}</h1><p>${escapedMessage}</p></main></body></html>`,
    {
      status,
      headers: {
        "Content-Type": "text/html; charset=utf-8",
      },
    }
  )
}

function escapeHtml(value: string): string {
  return value
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;")
    .replaceAll("'", "&#39;")
}
