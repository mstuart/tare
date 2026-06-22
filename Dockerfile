# --- build stage ---
# tree-sitter grammars compile C, so the builder needs a C toolchain.
FROM rust:1.82-slim AS builder
RUN apt-get update \
    && apt-get install -y --no-install-recommends gcc libc6-dev \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY . .
RUN cargo build --release -p tare-proxy

# --- runtime stage ---
FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd -m -u 10001 tare
COPY --from=builder /app/target/release/tare-proxy /usr/local/bin/tare-proxy
USER tare
ENV TARE_PORT=8787 \
    TARE_UPSTREAM=https://api.anthropic.com
EXPOSE 8787
ENTRYPOINT ["tare-proxy"]
