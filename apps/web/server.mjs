import http from "node:http"
import { createReadStream } from "node:fs"
import { stat } from "node:fs/promises"
import path from "node:path"
import { Readable } from "node:stream"
import { fileURLToPath, pathToFileURL } from "node:url"

const __dirname = path.dirname(fileURLToPath(import.meta.url))
const clientRoot = path.join(__dirname, "dist", "client")
const serverEntry = await import(pathToFileURL(path.join(__dirname, "dist", "server", "server.js")))
const handler = serverEntry.default

const port = Number(process.env.PORT || process.env.BINDHUB_WEB_PORT || 3000)
const host = process.env.HOST || "0.0.0.0"

const contentTypes = new Map([
  [".css", "text/css; charset=utf-8"],
  [".html", "text/html; charset=utf-8"],
  [".ico", "image/x-icon"],
  [".js", "text/javascript; charset=utf-8"],
  [".json", "application/json; charset=utf-8"],
  [".map", "application/json; charset=utf-8"],
  [".png", "image/png"],
  [".svg", "image/svg+xml"],
  [".txt", "text/plain; charset=utf-8"],
  [".webmanifest", "application/manifest+json"],
  [".woff2", "font/woff2"],
])

function isStaticPath(pathname) {
  return (
    pathname.startsWith("/assets/") ||
    pathname === "/favicon.ico" ||
    pathname === "/manifest.json" ||
    pathname === "/robots.txt"
  )
}

async function tryServeStatic(req, res, pathname) {
  if (!isStaticPath(pathname)) return false

  const decoded = decodeURIComponent(pathname)
  const filePath = path.normalize(path.join(clientRoot, decoded))
  const relative = path.relative(clientRoot, filePath)
  if (relative.startsWith("..") || path.isAbsolute(relative)) {
    res.writeHead(400)
    res.end("Bad request")
    return true
  }

  try {
    const info = await stat(filePath)
    if (!info.isFile()) return false
    const headers = {
      "content-length": String(info.size),
      "content-type": contentTypes.get(path.extname(filePath)) || "application/octet-stream",
    }
    if (pathname.startsWith("/assets/")) {
      headers["cache-control"] = "public, max-age=31536000, immutable"
    }
    res.writeHead(200, headers)
    if (req.method === "HEAD") {
      res.end()
      return true
    }
    createReadStream(filePath).pipe(res)
    return true
  } catch {
    return false
  }
}

function requestFromNode(req) {
  const url = new URL(req.url || "/", `http://${req.headers.host || `localhost:${port}`}`)
  const init = {
    method: req.method,
    headers: req.headers,
  }
  if (req.method !== "GET" && req.method !== "HEAD") {
    init.body = req
    init.duplex = "half"
  }
  return new Request(url, init)
}

async function sendWebResponse(res, response) {
  res.statusCode = response.status
  response.headers.forEach((value, key) => {
    res.setHeader(key, value)
  })

  if (!response.body) {
    res.end()
    return
  }

  Readable.fromWeb(response.body).pipe(res)
}

const server = http.createServer(async (req, res) => {
  try {
    const url = new URL(req.url || "/", `http://${req.headers.host || `localhost:${port}`}`)
    if (await tryServeStatic(req, res, url.pathname)) return

    const response = await handler.fetch(requestFromNode(req))
    await sendWebResponse(res, response)
  } catch (error) {
    console.error("[bindhub-web] request failed", error)
    if (!res.headersSent) {
      res.writeHead(500, { "content-type": "text/plain; charset=utf-8" })
    }
    res.end("Internal Server Error")
  }
})

server.listen(port, host, () => {
  console.log(`bindhub web listening on http://${host}:${port}`)
})
