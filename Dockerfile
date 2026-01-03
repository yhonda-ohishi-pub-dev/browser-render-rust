# Stage 1: Build Rust binary
FROM rust:1.85-slim-bookworm AS builder

RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    libsqlite3-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build

# Copy Cargo files first for dependency caching
COPY Cargo.toml Cargo.lock ./

# Create dummy src to build dependencies
RUN mkdir -p src && \
    echo "fn main() {}" > src/main.rs && \
    echo "pub fn lib() {}" > src/lib.rs

# Build dependencies (cached layer)
RUN cargo build --release && rm -rf src

# Copy actual source code
COPY src ./src

# Touch main.rs to invalidate cache for source files only
RUN touch src/main.rs && cargo build --release

# Stage 2: Chromium + dependencies
FROM debian:bookworm-slim AS chromium

RUN apt-get update && apt-get install -y --no-install-recommends \
    chromium \
    libsqlite3-0 \
    libssl3 \
    ca-certificates \
    fonts-liberation \
    libasound2 \
    libatk-bridge2.0-0 \
    libatk1.0-0 \
    libcups2 \
    libdbus-1-3 \
    libdrm2 \
    libgbm1 \
    libgtk-3-0 \
    libnspr4 \
    libnss3 \
    libxcomposite1 \
    libxdamage1 \
    libxfixes3 \
    libxkbcommon0 \
    libxrandr2 \
    && rm -rf /var/lib/apt/lists/*

# Stage 3: Distroless runtime
FROM gcr.io/distroless/cc-debian12:nonroot

WORKDIR /app

# Copy binary from builder
COPY --from=builder /build/target/release/browser-render .

# Copy Chromium and its dependencies
COPY --from=chromium /usr/bin/chromium /usr/bin/chromium
COPY --from=chromium /usr/lib/chromium /usr/lib/chromium
COPY --from=chromium /usr/share/chromium /usr/share/chromium
COPY --from=chromium /etc/chromium /etc/chromium

# Copy shared libraries required by the binary and Chromium
COPY --from=chromium /lib/x86_64-linux-gnu /lib/x86_64-linux-gnu
COPY --from=chromium /usr/lib/x86_64-linux-gnu /usr/lib/x86_64-linux-gnu

# Copy fonts for proper rendering
COPY --from=chromium /usr/share/fonts /usr/share/fonts
COPY --from=chromium /etc/fonts /etc/fonts

# Copy CA certificates for HTTPS
COPY --from=chromium /etc/ssl/certs /etc/ssl/certs

# Environment
ENV CHROME_PATH=/usr/bin/chromium
ENV CHROMIUM_PATH=/usr/bin/chromium

# Expose ports (HTTP and gRPC)
EXPOSE 8080 50051

# Run as nonroot user (UID 65532 in distroless)
CMD ["./browser-render", "--headless", "true", "--log-format", "json"]
