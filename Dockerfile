# Stage 1: Build Rust binary
FROM rust:1.85-slim-bookworm AS builder

# Limit parallel compilation jobs (default: 2)
ARG CARGO_BUILD_JOBS=2
ENV CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS}

RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    libsqlite3-dev \
    protobuf-compiler \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build

# Copy Cargo files and submodule first for dependency caching
COPY Cargo.toml Cargo.lock ./
COPY rust-scraper ./rust-scraper

# Create dummy src to build dependencies
RUN mkdir -p src && \
    echo "fn main() {}" > src/main.rs && \
    echo "pub fn lib() {}" > src/lib.rs

# Copy proto files for gRPC build
COPY proto ./proto
COPY build.rs ./

# Build dependencies (cached layer)
RUN cargo build --release --features grpc -j ${CARGO_BUILD_JOBS} && rm -rf src

# Copy actual source code
COPY src ./src

# Touch main.rs to invalidate cache for source files only
RUN touch src/main.rs && cargo build --release --features grpc -j ${CARGO_BUILD_JOBS}

# Stage 2: Runtime with headless-shell
FROM chromedp/headless-shell:stable AS headless

# Stage 3: Final runtime
FROM debian:bookworm-slim

# Install runtime dependencies only
RUN apt-get update && apt-get install -y --no-install-recommends \
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
    dumb-init \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd -r appuser && useradd -r -g appuser appuser

WORKDIR /app

# Copy binary from builder
COPY --from=builder /build/target/release/browser-render .

# Copy headless-shell from chromedp image
COPY --from=headless /headless-shell /headless-shell

# Environment - use headless-shell
ENV CHROME_PATH=/headless-shell/headless-shell
ENV CHROMIUM_PATH=/headless-shell/headless-shell

# Create data directories
RUN mkdir -p /app/data /app/logs && chown -R appuser:appuser /app

# Switch to non-root user
USER appuser

# Expose ports (HTTP and gRPC)
EXPOSE 8080 50051

# Use dumb-init to reap zombie processes
ENTRYPOINT ["/usr/bin/dumb-init", "--"]
CMD ["./browser-render", "--server", "both", "--log-format", "json"]
