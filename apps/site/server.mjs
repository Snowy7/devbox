import http from "node:http"
import { createReadStream } from "node:fs"
import { stat } from "node:fs/promises"
import path from "node:path"
import { fileURLToPath } from "node:url"

const __dirname = path.dirname(fileURLToPath(import.meta.url))
const distRoot = path.join(__dirname, "dist")
const port = Number(process.env.PORT || process.env.BINDHUB_SITE_PORT || 3002)
const host = process.env.HOST || "0.0.0.0"

const contentTypes = new Map([
  [".css", "text/css; charset=utf-8"],
  [".html", "text/html; charset=utf-8"],
  [".ico", "image/x-icon"],
  [".js", "text/javascript; charset=utf-8"],
  [".json", "application/json; charset=utf-8"],
  [".png", "image/png"],
  [".svg", "image/svg+xml"],
  [".txt", "text/plain; charset=utf-8"],
  [".woff2", "font/woff2"],
])

async function resolveFile(pathname) {
  const decoded = decodeURIComponent(pathname)
  const candidates = decoded.endsWith("/")
    ? [path.join(distRoot, decoded, "index.html")]
    : [path.join(distRoot, decoded), path.join(distRoot, decoded, "index.html")]

  for (const candidate of candidates) {
    const filePath = path.normalize(candidate)
    const relative = path.relative(distRoot, filePath)
    if (relative.startsWith("..") || path.isAbsolute(relative)) return null
    try {
      const info = await stat(filePath)
      if (info.isFile()) return { filePath, size: info.size }
    } catch {
      // Try the next candidate.
    }
  }
  return null
}

const server = http.createServer(async (req, res) => {
  try {
    const url = new URL(req.url || "/", `http://${req.headers.host || `localhost:${port}`}`)
    const file = await resolveFile(url.pathname)
    if (!file) {
      res.writeHead(404, { "content-type": "text/plain; charset=utf-8" })
      res.end("Not Found")
      return
    }

    res.writeHead(200, {
      "content-length": String(file.size),
      "content-type": contentTypes.get(path.extname(file.filePath)) || "application/octet-stream",
    })
    if (req.method === "HEAD") {
      res.end()
      return
    }
    createReadStream(file.filePath).pipe(res)
  } catch (error) {
    console.error("[bindhub-site] request failed", error)
    if (!res.headersSent) {
      res.writeHead(500, { "content-type": "text/plain; charset=utf-8" })
    }
    res.end("Internal Server Error")
  }
})

server.listen(port, host, () => {
  console.log(`bindhub site listening on http://${host}:${port}`)
})
