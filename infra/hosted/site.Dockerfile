FROM node:22-bookworm-slim AS builder

WORKDIR /app
RUN corepack enable

COPY package.json pnpm-lock.yaml pnpm-workspace.yaml turbo.json tsconfig.json ./
COPY apps ./apps
COPY packages ./packages

RUN pnpm install --frozen-lockfile
RUN pnpm --filter apps-site build
RUN pnpm deploy --filter apps-site --prod --legacy /runtime

FROM node:22-bookworm-slim

WORKDIR /app
ENV NODE_ENV=production
ENV PORT=3002

COPY --from=builder /runtime ./
COPY --from=builder /app/apps/site/dist ./dist
COPY --from=builder /app/apps/site/server.mjs ./server.mjs

EXPOSE 3002

CMD ["node", "server.mjs"]
