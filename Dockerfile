# Build stage
FROM rust:1.83-alpine AS builder

RUN apk add --no-cache musl-dev

WORKDIR /app

# Copy manifests first for better caching
COPY Cargo.toml Cargo.lock ./

# Create dummy src to build dependencies
RUN mkdir src && echo "fn main() {}" > src/main.rs
RUN cargo build --release && rm -rf src

# Copy actual source and rebuild
COPY src ./src
RUN touch src/main.rs && cargo build --release

# Runtime stage
FROM alpine:3.21

RUN apk add --no-cache ca-certificates

WORKDIR /app

COPY --from=builder /app/target/release/spawngate /usr/local/bin/spawngate

# Create non-root user
RUN addgroup -S spawngate && adduser -S spawngate -G spawngate
USER spawngate

# Default ports: HTTP, HTTPS, Admin
EXPOSE 80 443 9999

ENTRYPOINT ["spawngate"]
CMD ["config.toml"]
