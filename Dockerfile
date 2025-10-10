FROM rust:1.75 as builder

WORKDIR /app
COPY . .

# Build all binaries in release mode
RUN cargo build --release

# Use distroless for minimal runtime
FROM gcr.io/distroless/cc-debian12

# Copy all pandemic binaries
COPY --from=builder /app/target/release/pandemic /usr/local/bin/
COPY --from=builder /app/target/release/pandemic-cli /usr/local/bin/
COPY --from=builder /app/target/release/pandemic-udp /usr/local/bin/
COPY --from=builder /app/target/release/hello-infection /usr/local/bin/

# Create runtime directory for sockets
RUN mkdir -p /var/run/pandemic

# Default to running the daemon
ENTRYPOINT ["/usr/local/bin/pandemic"]