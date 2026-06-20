FROM rust:1.88-bookworm AS builder

WORKDIR /app
COPY . .
RUN cargo build --release --locked -p devbox-api

FROM debian:bookworm-slim

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && mkdir -p /data/devbox-api \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/devbox-api /usr/local/bin/devbox-api

ENV DEVBOX_API_ROOT=/data/devbox-api

EXPOSE 8787

CMD ["devbox-api"]
