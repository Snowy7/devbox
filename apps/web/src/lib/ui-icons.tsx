import {
  Cloud,
  CloudDownload,
  Globe,
  HardDrive,
  Lock,
  ShieldAlert,
  ShieldCheck,
  Users,
  type LucideIcon,
} from "lucide-react"

import type {
  MachineSummary,
  SharedFolderSummary,
} from "@/lib/dashboard-api"

export function visibilityIcon(
  visibility: SharedFolderSummary["visibility"]
): LucideIcon {
  switch (visibility) {
    case "public":
      return Globe
    case "team":
      return Users
    default:
      return Lock
  }
}

export function hydrationIcon(
  state: SharedFolderSummary["hydrationState"]
): LucideIcon {
  switch (state) {
    case "fully-local":
      return HardDrive
    case "partial":
      return CloudDownload
    default:
      return Cloud
  }
}

export function trustIcon(state: MachineSummary["trustState"]): LucideIcon {
  return state === "trusted" ? ShieldCheck : ShieldAlert
}

export function trustIconClass(state: MachineSummary["trustState"]) {
  return state === "trusted" ? "text-signal" : "text-faint"
}