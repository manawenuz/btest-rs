# Docker and Deployment Guide

## Container Registries

Images are published to:

```
git.manko.yoga/manawenuz/btest-rs    # Gitea registry
ghcr.io/manawenuz/btest-rs           # GitHub Container Registry
```

## Quick Start

### Docker Compose (recommended)

```bash
# Server with no authentication
docker compose up -d

# Server with authentication
docker compose --profile auth up -d

# View logs
docker compose logs -f
```

### One-liner server

```bash
docker build -t btest-rs . && \
docker run --rm -it \
  -p 2000:2000/tcp \
  -p 2001-2100:2001-2100/udp \
  -p 2257-2356:2257-2356/udp \
  btest-rs -s -v
```

### One-liner server with authentication

```bash
docker run --rm -it \
  -p 2000:2000/tcp \
  -p 2001-2100:2001-2100/udp \
  -p 2257-2356:2257-2356/udp \
  btest-rs -s -a admin -p password -v
```

### Server with EC-SRP5 authentication

```bash
docker run --rm -it \
  -p 2000:2000/tcp \
  -p 2001-2100:2001-2100/udp \
  -p 2257-2356:2257-2356/udp \
  btest-rs -s -a admin -p password --ecsrp5 -v
```

### Server with syslog and CSV

```bash
docker run --rm -it \
  -p 2000:2000/tcp \
  -p 2001-2100:2001-2100/udp \
  -p 2257-2356:2257-2356/udp \
  -v /var/log/btest:/data \
  btest-rs -s -a admin -p password --syslog 192.168.1.1:514 --csv /data/results.csv -v
```

### Client mode

```bash
# TCP download test against MikroTik
docker run --rm -it btest-rs -c 192.168.88.1 -r

# UDP bidirectional
docker run --rm -it btest-rs -c 192.168.88.1 -t -r -u

# Timed test with CSV output
docker run --rm -it \
  -v $(pwd):/data \
  btest-rs -c 192.168.88.1 -r -d 30 --csv /data/results.csv

# With authentication
docker run --rm -it btest-rs -c 192.168.88.1 -r -a admin -p password
```

### Using pre-built image from registry

```bash
# Pull from Gitea registry
docker pull git.manko.yoga/manawenuz/btest-rs:latest

# Run server
docker run --rm -it \
  -p 2000:2000/tcp \
  -p 2001-2100:2001-2100/udp \
  -p 2257-2356:2257-2356/udp \
  git.manko.yoga/manawenuz/btest-rs:latest -s -v
```

## Docker Compose

The `docker-compose.yml` file provides two service profiles:

### Default profile (no auth)

```bash
docker compose up -d
```

Starts a server on port 2000 with verbose logging and no authentication.

### Auth profile

```bash
docker compose --profile auth up -d
```

Starts an additional server on port 2010 with MD5 authentication (user: admin, password: password).

### docker-compose.yml

```yaml
services:
  btest-server:
    build: .
    image: git.manko.yoga/manawenuz/btest-rs:latest
    container_name: btest-server
    ports:
      - "2000:2000/tcp"
      - "2001-2100:2001-2100/udp"
      - "2257-2356:2257-2356/udp"
    command: ["-s", "-v"]
    restart: unless-stopped

  btest-server-auth:
    build: .
    image: git.manko.yoga/manawenuz/btest-rs:latest
    container_name: btest-server-auth
    ports:
      - "2010:2000/tcp"
      - "2101-2200:2001-2100/udp"
    command: ["-s", "-a", "admin", "-p", "password", "-v"]
    restart: unless-stopped
    profiles:
      - auth
```

## Dockerfile

The production Dockerfile uses a multi-stage build:

1. **Build stage** -- Rust 1.86 slim image, compiles a release binary
2. **Runtime stage** -- Debian Bookworm slim, copies only the binary

The resulting image is approximately 80 MB. The binary itself is about 2 MB.

Exposed ports:
- `2000/tcp` -- control channel
- `2001-2100/udp` -- server-side data ports
- `2257-2356/udp` -- client-side data ports

Default entrypoint: `btest -s`

## Building Images

### Local build (native)

```bash
cargo build --release
# Binary at: target/release/btest
```

### Cross-compile for Linux x86_64 (from macOS)

```bash
scripts/build-linux.sh
# Binary at: dist/btest (static musl, ~2 MB)
```

### Docker image build

```bash
# Production image
docker build -t btest-rs .

# With custom tag
docker build -t git.manko.yoga/manawenuz/btest-rs:latest .
docker build -t git.manko.yoga/manawenuz/btest-rs:0.5.0 .
```

### Multi-platform build

```bash
docker buildx build \
  --platform linux/amd64,linux/arm64 \
  -t git.manko.yoga/manawenuz/btest-rs:latest \
  --push .
```

## Push to Registry

```bash
# Login to Gitea registry
docker login git.manko.yoga

# Tag and push
docker build -t git.manko.yoga/manawenuz/btest-rs:latest .
docker push git.manko.yoga/manawenuz/btest-rs:latest

# Also tag with version
docker tag git.manko.yoga/manawenuz/btest-rs:latest \
           git.manko.yoga/manawenuz/btest-rs:0.5.0
docker push git.manko.yoga/manawenuz/btest-rs:0.5.0
```

## Deployment Options

### Option 1: Docker (single container)

```bash
docker run -d --name btest-server \
  --restart unless-stopped \
  -p 2000:2000/tcp \
  -p 2001-2100:2001-2100/udp \
  -p 2257-2356:2257-2356/udp \
  git.manko.yoga/manawenuz/btest-rs:latest \
  -s -a admin -p password --ecsrp5 -v
```

### Option 2: Static binary + systemd

```bash
# Copy binary to server
scp dist/btest root@server:/usr/local/bin/btest

# Run the installer
scp scripts/install-service.sh root@server:/tmp/
ssh root@server "bash /tmp/install-service.sh --auth-user admin --auth-pass password"
```

The installer script:
- Creates a dedicated `btest` system user
- Installs a hardened systemd unit with security options (NoNewPrivileges, ProtectSystem, PrivateTmp)
- Grants `CAP_NET_BIND_SERVICE` for binding to ports below 1024
- Enables and starts the service
- Supports `--auth-user`, `--auth-pass`, and `--port` options

Useful systemd commands after installation:

```bash
systemctl status btest       # Check status
systemctl stop btest         # Stop the service
systemctl restart btest      # Restart
journalctl -u btest -f       # Follow logs
systemctl disable btest      # Disable autostart
```

### Option 3: Docker Compose on server

```bash
scp docker-compose.yml root@server:/opt/btest-rs/
ssh root@server "cd /opt/btest-rs && docker compose up -d"
```

## Port Reference

| Port | Protocol | Purpose |
|------|----------|---------|
| 2000 | TCP | Control channel (handshake, auth, status exchange) |
| 2001-2100 | UDP | Server-side data ports |
| 2257-2356 | UDP | Client-side data ports (server_port + 256) |

### Firewall rules (iptables)

```bash
iptables -A INPUT -p tcp --dport 2000 -j ACCEPT
iptables -A INPUT -p udp --dport 2001:2100 -j ACCEPT
iptables -A INPUT -p udp --dport 2257:2356 -j ACCEPT
```

### Firewall rules (ufw)

```bash
ufw allow 2000/tcp
ufw allow 2001:2100/udp
ufw allow 2257:2356/udp
```

### Firewall rules (nftables)

```bash
nft add rule inet filter input tcp dport 2000 accept
nft add rule inet filter input udp dport 2001-2100 accept
nft add rule inet filter input udp dport 2257-2356 accept
```

## Health Check

```bash
# Check if server is responding (TCP handshake)
nc -zv <server-ip> 2000

# Check Docker container status
docker logs btest-server
docker ps --filter name=btest-server

# Check systemd service
systemctl status btest
journalctl -u btest --since "5 minutes ago"
```

## Resource Usage

| Resource | Value |
|----------|-------|
| Memory (idle) | ~5 MB |
| Memory (per active connection) | +1 MB |
| CPU | Minimal when idle, scales with bandwidth |
| Binary size | ~2 MB (static musl build) |
| Docker image | ~80 MB (Debian slim + binary) |
