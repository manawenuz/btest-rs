# Docker & Deployment Guide

## Container Registry

Images are published to:
```
git.manko.yoga/manawenuz/btest-rs
```

## Quick Run (Ephemeral)

### Server (one-liner)

```bash
# Build and run server directly
docker build -t btest-rs . && \
docker run --rm -it \
  -p 2000:2000/tcp \
  -p 2001-2100:2001-2100/udp \
  -p 2257-2356:2257-2356/udp \
  btest-rs -s -v

# With authentication
docker run --rm -it \
  -p 2000:2000/tcp \
  -p 2001-2100:2001-2100/udp \
  -p 2257-2356:2257-2356/udp \
  btest-rs -s -a admin -p password -v
```

### Client (one-liner)

```bash
# TCP download test against MikroTik
docker run --rm -it btest-rs -c 192.168.88.1 -r

# UDP bidirectional
docker run --rm -it btest-rs -c 192.168.88.1 -t -r -u
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

### Basic server

```bash
docker compose up -d
```

### Server with authentication

```bash
docker compose --profile auth up -d
```

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

## Building

### Local build (native)

```bash
cargo build --release
# Binary at: target/release/btest
```

### Cross-compile for Linux x86_64 (from macOS)

```bash
scripts/build-linux.sh
# Binary at: dist/btest (static musl, 2 MB)
```

### Docker image build

```bash
# Production image (for running)
docker build -t btest-rs .

# With custom tag
docker build -t git.manko.yoga/manawenuz/btest-rs:latest .
docker build -t git.manko.yoga/manawenuz/btest-rs:0.1.0 .
```

### Multi-platform build

```bash
# Build for both ARM64 and x86_64
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
           git.manko.yoga/manawenuz/btest-rs:0.1.0
docker push git.manko.yoga/manawenuz/btest-rs:0.1.0
```

## Deployment on Linux Server

### Option 1: Docker

```bash
docker run -d --name btest-server \
  --restart unless-stopped \
  -p 2000:2000/tcp \
  -p 2001-2100:2001-2100/udp \
  -p 2257-2356:2257-2356/udp \
  git.manko.yoga/manawenuz/btest-rs:latest \
  -s -a admin -p password -v
```

### Option 2: Static binary + systemd

```bash
# Copy binary to server
scp dist/btest root@server:/usr/local/bin/btest

# Copy and run installer
scp scripts/install-service.sh root@server:/tmp/
ssh root@server "bash /tmp/install-service.sh --auth-user admin --auth-pass password"
```

### Option 3: Docker Compose on server

```bash
scp docker-compose.yml root@server:/opt/btest-rs/
ssh root@server "cd /opt/btest-rs && docker compose up -d"
```

## Port Reference

| Port | Protocol | Purpose |
|------|----------|---------|
| 2000 | TCP | Control channel (handshake, auth, status) |
| 2001-2100 | UDP | Server-side data ports |
| 2257-2356 | UDP | Client-side data ports (2001+256) |

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

## Health Check

```bash
# Check if server is responding
nc -zv <server-ip> 2000

# Check Docker container
docker logs btest-server
docker exec btest-server ps aux
```

## Resource Usage

- **Memory**: ~5 MB base, +1 MB per active connection
- **CPU**: Minimal when idle, scales with bandwidth
- **Binary size**: 2 MB (static musl build)
- **Docker image**: ~80 MB (Debian slim + binary)
