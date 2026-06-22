# --- build stage ---
# tree-sitter grammars compile C, so the builder needs a C toolchain.
FROM rust:1.82-slim AS builder
RUN apt-get update \
    && apt-get install -y --no-install-recommends gcc libc6-dev \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY . .
RUN cargo build --release -p cull-proxy

# --- runtime stage ---
FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd -m -u 10001 cull
COPY --from=builder /app/target/release/cull-proxy /usr/local/bin/cull-proxy
USER cull
ENV CULL_PORT=8787 \
    CULL_UPSTREAM=https://api.anthropic.com
EXPOSE 8787
ENTRYPOINT ["cull-proxy"]
