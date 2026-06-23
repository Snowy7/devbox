import fs from "node:fs"
import path from "node:path"

import { readRuntimeEnv, type BindhubRuntimeEnv } from "@/lib/auth"

export function readServerRuntimeEnv(): BindhubRuntimeEnv {
  return {
    ...readLocalEnvFile(),
    ...readRuntimeEnv(),
    ...process.env,
  }
}

function readLocalEnvFile(): BindhubRuntimeEnv {
  const candidates = [
    path.join(process.cwd(), ".env.local"),
    path.join(process.cwd(), "apps", "web", ".env.local"),
  ]

  for (const candidate of candidates) {
    if (fs.existsSync(candidate)) {
      return parseEnvFile(fs.readFileSync(candidate, "utf8"))
    }
  }

  return {}
}

function parseEnvFile(contents: string): BindhubRuntimeEnv {
  const env: BindhubRuntimeEnv = {}

  for (const rawLine of contents.split(/\r?\n/)) {
    const line = rawLine.trim()

    if (!line || line.startsWith("#")) {
      continue
    }

    const equals = line.indexOf("=")

    if (equals <= 0) {
      continue
    }

    const key = line.slice(0, equals).trim()
    let value = line.slice(equals + 1).trim()

    if (
      (value.startsWith('"') && value.endsWith('"')) ||
      (value.startsWith("'") && value.endsWith("'"))
    ) {
      value = value.slice(1, -1)
    }

    env[key] = value
  }

  return env
}
