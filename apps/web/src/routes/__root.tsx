import {
  HeadContent,
  Outlet,
  Scripts,
  createRootRoute,
} from "@tanstack/react-router"
import { AuthKitProvider } from "@workos/authkit-tanstack-react-start/client"

import appCss from "@workspace/ui/globals.css?url"

export const Route = createRootRoute({
  head: () => ({
    meta: [
      {
        charSet: "utf-8",
      },
      {
        name: "viewport",
        content: "width=device-width, initial-scale=1",
      },
      {
        title: "Bindhub",
      },
    ],
    links: [
      {
        rel: "stylesheet",
        href: appCss,
      },
    ],
  }),
  notFoundComponent: () => (
    <main className="container mx-auto p-4 pt-16">
      <h1 className="text-2xl font-semibold">404</h1>
      <p className="text-sm text-muted-foreground">
        The requested page could not be found.
      </p>
    </main>
  ),
  component: RootComponent,
  shellComponent: RootDocument,
})

function RootComponent() {
  return (
    <AuthKitProvider>
      <Outlet />
    </AuthKitProvider>
  )
}

function RootDocument({ children }: { children: React.ReactNode }) {
  return (
    <html lang="en" suppressHydrationWarning>
      <head>
        <script
          dangerouslySetInnerHTML={{
            __html: `
              try {
                const theme = localStorage.getItem("Bindhub-theme") || "dark";
                const light = theme === "light" || (theme === "system" && !window.matchMedia("(prefers-color-scheme: dark)").matches);
                document.documentElement.classList.toggle("light", light);
              } catch {}
            `,
          }}
        />
        <HeadContent />
      </head>
      <body className="min-h-svh bg-background text-foreground">
        {children}
        <Scripts />
      </body>
    </html>
  )
}