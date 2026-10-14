# Optional: Multi-stage Docker build for platforms that require containers
FROM rust:1.80-slim AS builder

WORKDIR /app

# Copy repo files
COPY . .

# Build release binaries for both the shell and server
RUN cargo build --release -p rust_shell -p server

# Runtime stage
FROM debian:bookworm-slim

WORKDIR /app

RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/*

# Copy binaries and prebuilt frontend
COPY --from=builder /app/target/release/server /app/server
COPY --from=builder /app/target/release/rust_shell /app/rust_shell
COPY --from=builder /app/dist /app/dist

ENV PORT=3000
ENV SHELL_BINARY_PATH=/app/rust_shell
ENV STORAGE_DIR=/app/storage/users

EXPOSE 3000

CMD ["/app/server"]
