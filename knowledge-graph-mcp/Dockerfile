# Multi-stage build for optimal image size
FROM rust:1.75-alpine AS builder

# Install build dependencies
RUN apk add --no-cache musl-dev openssl-dev pkgconfig

# Create app directory
WORKDIR /app

# Copy manifest files
COPY Cargo.toml Cargo.lock ./

# Build dependencies (this is cached if Cargo files don't change)
RUN mkdir src && \
    echo "fn main() {}" > src/main.rs && \
    cargo build --release && \
    rm -rf src

# Copy source code
COPY src ./src

# Build the application
RUN cargo build --release --bin knowledge-graph-mcp

# Runtime stage
FROM alpine:3.19

# Install runtime dependencies
RUN apk add --no-cache \
    libgcc \
    libstdc++ \
    ca-certificates \
    tini

# Create non-root user
RUN addgroup -g 1000 mcp && \
    adduser -D -s /bin/sh -u 1000 -G mcp mcp

# Copy binary from builder
COPY --from=builder /app/target/release/knowledge-graph-mcp /usr/local/bin/

# Copy configuration
COPY config /config

# Set ownership
RUN chown -R mcp:mcp /config

# Switch to non-root user
USER mcp

# Set working directory
WORKDIR /workspace

# Use tini as entrypoint for proper signal handling
ENTRYPOINT ["/sbin/tini", "--"]

# Run the MCP server
CMD ["knowledge-graph-mcp"]