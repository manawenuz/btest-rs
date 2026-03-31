# Stage 1: Build
FROM rust:1.82-slim AS builder

WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY src/ src/

RUN cargo build --release

# Stage 2: Runtime (minimal image)
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /build/target/release/btest /usr/local/bin/btest

# btest control port
EXPOSE 2000/tcp
# UDP data ports range
EXPOSE 2001-2100/udp
# UDP client ports range
EXPOSE 2257-2356/udp

ENTRYPOINT ["btest"]
# Default: run as server
CMD ["-s"]
