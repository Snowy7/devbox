"use client"

import {
  Database,
  LogOut,
  Mail,
  Monitor,
  Moon,
  Palette,
  Settings,
  Sun,
  User,
  UserCircle,
  type LucideIcon,
} from "lucide-react"
import { useEffect, useState } from "react"

import { Button } from "@workspace/ui/components/button"
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@workspace/ui/components/dialog"
import { Label } from "@workspace/ui/components/label"
import { Separator } from "@workspace/ui/components/separator"

import { authRoutes } from "@/lib/auth"
import type { DashboardData } from "@/lib/dashboard-api"

type SettingsSection = "profile" | "appearance" | "account"

type Theme = "dark" | "light" | "system"

type SettingsModalProps = {
  open: boolean
  onOpenChange: (open: boolean) => void
  data: DashboardData
}

const sections: {
  id: SettingsSection
  label: string
  icon: LucideIcon
}[] = [
  { id: "profile", label: "Profile", icon: UserCircle },
  { id: "appearance", label: "Appearance", icon: Palette },
  { id: "account", label: "Account", icon: User },
]

export function SettingsModal({
  open,
  onOpenChange,
  data,
}: SettingsModalProps) {
  const [section, setSection] = useState<SettingsSection>("profile")
  const [theme, setTheme] = useState<Theme>("dark")

  useEffect(() => {
    if (!open) return
    setTheme(readStoredTheme())
  }, [open])

  function selectTheme(next: Theme) {
    localStorage.setItem("Bindhub-theme", next)
    setTheme(next)
    applyTheme(next)
  }

  const initials = data.identity.account.displayName
    .split(/\s+/)
    .map((part) => part[0])
    .join("")
    .slice(0, 2)
    .toUpperCase()

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent
        className="flex h-[min(560px,90vh)] max-w-2xl flex-col gap-0 overflow-hidden p-0 sm:max-w-2xl"
        showCloseButton
      >
        <DialogHeader className="border-b border-divider px-6 py-4">
          <DialogTitle className="flex items-center gap-2">
            <Settings className="size-4 text-muted-foreground" />
            Settings
          </DialogTitle>
          <DialogDescription>
            Profile, appearance, and account preferences.
          </DialogDescription>
        </DialogHeader>

        <div className="flex min-h-0 flex-1">
          <nav className="flex w-44 shrink-0 flex-col gap-0.5 border-r border-divider bg-muted/30 p-2">
            {sections.map((item) => {
              const Icon = item.icon
              return (
                <button
                  key={item.id}
                  type="button"
                  onClick={() => setSection(item.id)}
                  className={`flex items-center gap-2 px-3 py-2 text-left text-sm transition-colors ${
                    section === item.id
                      ? "bg-muted font-medium text-foreground"
                      : "text-muted-foreground hover:bg-muted hover:text-foreground"
                  }`}
                >
                  <Icon className="size-4 shrink-0" />
                  {item.label}
                </button>
              )
            })}
          </nav>

          <div className="min-w-0 flex-1 overflow-y-auto p-6">
            {section === "profile" ? (
              <ProfileSettings
                initials={initials}
                displayName={data.identity.account.displayName}
                email={data.identity.account.email}
              />
            ) : null}

            {section === "appearance" ? (
              <AppearanceSettings theme={theme} onSelectTheme={selectTheme} />
            ) : null}

            {section === "account" ? (
              <AccountSettings sourceLabel={data.source.label} />
            ) : null}
          </div>
        </div>
      </DialogContent>
    </Dialog>
  )
}

function ProfileSettings({
  initials,
  displayName,
  email,
}: {
  initials: string
  displayName: string
  email: string
}) {
  return (
    <div className="space-y-6">
      <div>
        <h3 className="text-base font-semibold">Profile</h3>
        <p className="mt-1 text-sm text-muted-foreground">
          Your Bindhub identity from WorkOS.
        </p>
      </div>

      <div className="flex items-center gap-4">
        <div className="grid size-16 place-items-center border border-border bg-secondary text-lg font-semibold text-secondary-foreground">
          {initials || "DB"}
        </div>
        <div className="min-w-0">
          <p className="truncate font-medium">{displayName}</p>
          <p className="truncate text-sm text-muted-foreground">{email}</p>
        </div>
      </div>

      <Separator />

      <div className="space-y-4">
        <FieldReadonly icon={User} label="Display name" value={displayName} />
        <FieldReadonly icon={Mail} label="Email" value={email} />
      </div>
    </div>
  )
}

function AppearanceSettings({
  theme,
  onSelectTheme,
}: {
  theme: Theme
  onSelectTheme: (theme: Theme) => void
}) {
  const options: { id: Theme; label: string; icon: typeof Sun }[] = [
    { id: "dark", label: "Dark", icon: Moon },
    { id: "light", label: "Light", icon: Sun },
    { id: "system", label: "System", icon: Monitor },
  ]

  return (
    <div className="space-y-6">
      <div>
        <h3 className="text-base font-semibold">Appearance</h3>
        <p className="mt-1 text-sm text-muted-foreground">
          Choose how Bindhub looks on this device.
        </p>
      </div>

      <div className="grid gap-2">
        {options.map((option) => {
          const Icon = option.icon
          const active = theme === option.id

          return (
            <button
              key={option.id}
              type="button"
              onClick={() => onSelectTheme(option.id)}
              className={`flex items-center gap-3 border px-4 py-3 text-left text-sm transition-colors ${
                active
                  ? "border-signal bg-muted text-foreground"
                  : "border-border bg-card text-muted-foreground hover:border-muted-foreground hover:bg-muted hover:text-foreground"
              }`}
            >
              <Icon className="size-4 shrink-0" />
              <span className="font-medium">{option.label}</span>
            </button>
          )
        })}
      </div>
    </div>
  )
}

function AccountSettings({ sourceLabel }: { sourceLabel: string }) {
  return (
    <div className="space-y-6">
      <div>
        <h3 className="text-base font-semibold">Account</h3>
        <p className="mt-1 text-sm text-muted-foreground">
          Session and data source for this dashboard.
        </p>
      </div>

      <FieldReadonly icon={Database} label="Data source" value={sourceLabel} />

      <Separator />

      <div className="space-y-3">
        <Label>Session</Label>
        <Button asChild variant="outline" className="w-full justify-start">
          <a href={authRoutes.signOut}>
            <LogOut />
            Sign out
          </a>
        </Button>
      </div>
    </div>
  )
}

function FieldReadonly({
  icon: Icon,
  label,
  value,
}: {
  icon?: LucideIcon
  label: string
  value: string
}) {
  return (
    <div className="space-y-2">
      <Label className="inline-flex items-center gap-1.5">
        {Icon ? <Icon className="size-3.5 text-faint" /> : null}
        {label}
      </Label>
      <div className="border border-border bg-input px-3 py-2 text-sm text-muted-foreground">
        {value}
      </div>
    </div>
  )
}

function readStoredTheme(): Theme {
  const stored = localStorage.getItem("Bindhub-theme")
  return stored === "light" || stored === "dark" || stored === "system"
    ? stored
    : "dark"
}

function applyTheme(theme: Theme) {
  const isLight =
    theme === "light" ||
    (theme === "system" &&
      !window.matchMedia("(prefers-color-scheme: dark)").matches)

  document.documentElement.classList.toggle("light", isLight)
}