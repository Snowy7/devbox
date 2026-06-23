import type { ReactNode } from "react"

import { Link } from "@tanstack/react-router"
import { FolderOpen, KeyRound } from "lucide-react"

import { Button } from "@workspace/ui/components/button"
import { Card, CardContent, CardHeader, CardTitle } from "@workspace/ui/components/card"

type AuthScreenProps = {
  title: string
  description: string
  asideTitle: string
  asideDescription: string
  children: ReactNode
}

export function AuthScreen({
  title,
  description,
  asideTitle,
  asideDescription,
  children,
}: AuthScreenProps) {
  return (
    <main className="flex min-h-svh items-center justify-center bg-background p-6 text-foreground">
      <div className="grid w-full max-w-5xl gap-10 lg:grid-cols-[minmax(0,1fr)_400px]">
        <div className="space-y-6">
          <Link
            to="/"
            className="inline-flex items-center gap-2 text-base font-semibold text-foreground"
          >
            <span className="grid size-8 place-items-center border border-divider bg-input">
              <FolderOpen className="size-4 text-signal" />
            </span>
            Bindhub
          </Link>

          <div className="space-y-3">
            <h1 className="max-w-lg text-3xl font-semibold leading-tight">
              {title}
            </h1>
            <p className="max-w-md text-base leading-7 text-muted-foreground">
              {description}
            </p>
          </div>
        </div>

        <Card markers className="w-full">
          <CardHeader className="border-b border-divider">
            <div className="flex items-start gap-3">
              <div className="grid size-9 shrink-0 place-items-center border border-divider bg-input">
                <KeyRound className="size-4 text-muted-foreground" />
              </div>
              <div className="min-w-0">
                <CardTitle className="text-base">{asideTitle}</CardTitle>
                <p className="mt-1 text-sm text-muted-foreground">
                  {asideDescription}
                </p>
              </div>
            </div>
          </CardHeader>
          <CardContent className="py-5">{children}</CardContent>
        </Card>
      </div>
    </main>
  )
}

export function AuthSubmitButton({ children }: { children: ReactNode }) {
  return (
    <Button type="submit" className="h-9 w-full">
      {children}
    </Button>
  )
}

export function GoogleAuthButton({ href }: { href: string }) {
  return (
    <Button asChild variant="outline" className="h-9 w-full">
      <a href={href}>
        <GoogleMark />
        Continue with Google
      </a>
    </Button>
  )
}

export function AuthError({ message }: { message: string }) {
  return (
    <p className="border border-destructive bg-destructive/10 px-3 py-2 text-sm text-destructive">
      {message}
    </p>
  )
}

function GoogleMark() {
  return (
    <svg aria-hidden="true" className="size-4" viewBox="0 0 24 24">
      <path
        fill="#4285F4"
        d="M22.56 12.25c0-.78-.07-1.53-.2-2.25H12v4.26h5.92c-.26 1.37-1.04 2.53-2.21 3.31v2.77h3.57c2.08-1.92 3.28-4.74 3.28-8.09z"
      />
      <path
        fill="#34A853"
        d="M12 23c2.97 0 5.46-.98 7.28-2.66l-3.57-2.77c-.98.66-2.23 1.06-3.71 1.06-2.86 0-5.29-1.93-6.16-4.53H2.18v2.84C3.99 20.53 7.7 23 12 23z"
      />
      <path
        fill="#FBBC05"
        d="M5.84 14.09c-.22-.66-.35-1.36-.35-2.09s.13-1.43.35-2.09V7.07H2.18C1.43 8.55 1 10.22 1 12s.43 3.45 1.18 4.93l3.66-2.84z"
      />
      <path
        fill="#EA4335"
        d="M12 5.38c1.62 0 3.06.56 4.21 1.64l3.15-3.15C17.45 2.09 14.97 1 12 1 7.7 1 3.99 3.47 2.18 7.07l3.66 2.84c.87-2.6 3.3-4.53 6.16-4.53z"
      />
    </svg>
  )
}