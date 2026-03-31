#!/usr/bin/env python3
"""
Full MITM proxy for btest - forwards TCP control + UDP data.
Captures and logs ALL traffic between MikroTik client and MikroTik server.

Usage:
    python3 btest_mitm_full.py --target 172.16.81.1

Then on MikroTik:
    /tool/bandwidth-test address=<this_mac_ip> direction=receive protocol=tcp \
        user=antar password=antar connection-count=1
"""
import socket
import select
import sys
import argparse
import time
import threading
import struct


def ts():
    return time.strftime("%H:%M:%S", time.localtime()) + f".{int(time.time()*1000)%1000:03d}"


def hexline(data, offset=0, max_bytes=16):
    chunk = data[offset:offset+max_bytes]
    hex_part = " ".join(f"{b:02x}" for b in chunk)
    ascii_part = "".join(chr(b) if 32 <= b < 127 else "." for b in chunk)
    return f"  {offset:04x}  {hex_part:<48s}  {ascii_part}"


def log_data(direction, data, conn_id=""):
    label = f"[{ts()}] {direction}"
    if conn_id:
        label += f" [{conn_id}]"
    label += f" ({len(data)} bytes)"
    print(label)
    # Show first 4 lines of hex
    for i in range(0, min(len(data), 64), 16):
        print(hexline(data, i))
    if len(data) > 64:
        print(f"  ... ({len(data)} total)")

    # Try to annotate
    if len(data) == 4:
        val = data.hex()
        annotations = {
            "01000000": "HELLO / AUTH_OK",
            "02000000": "AUTH_REQUIRED (MD5)",
            "03000000": "AUTH_REQUIRED (EC-SRP5)",
            "00000000": "AUTH_FAILED",
        }
        if val in annotations:
            print(f"  >>> {annotations[val]}")

    if len(data) == 12 and data[0] == 0x07:
        # Status message
        seq = int.from_bytes(data[1:5], "big")
        recv_bytes = int.from_bytes(data[8:12], "little")
        mbps = recv_bytes * 8 / 1_000_000
        print(f"  >>> STATUS: seq={seq} bytes_received={recv_bytes} ({mbps:.2f} Mbps)")

    if len(data) == 16:
        proto = "UDP" if data[0] == 0 else "TCP"
        dirs = {1: "RX", 2: "TX", 3: "BOTH"}
        d = dirs.get(data[1], f"0x{data[1]:02x}")
        conn = data[3]
        print(f"  >>> COMMAND: proto={proto} dir={d} conn_count={conn}")

    sys.stdout.flush()


def proxy_tcp(client_sock, target_host, target_port, conn_id):
    """Proxy a single TCP connection."""
    try:
        server_sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        server_sock.settimeout(30)
        server_sock.connect((target_host, target_port))
        server_sock.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
    except Exception as e:
        print(f"[{conn_id}] Failed to connect to target: {e}")
        client_sock.close()
        return

    try:
        while True:
            readable, _, _ = select.select([client_sock, server_sock], [], [], 30)
            if not readable:
                break

            for sock in readable:
                if sock is server_sock:
                    data = server_sock.recv(65536)
                    if not data:
                        return
                    log_data("SERVER→CLIENT", data, conn_id)
                    client_sock.sendall(data)
                elif sock is client_sock:
                    data = client_sock.recv(65536)
                    if not data:
                        return
                    log_data("CLIENT→SERVER", data, conn_id)
                    server_sock.sendall(data)
    except Exception as e:
        print(f"[{conn_id}] Error: {e}")
    finally:
        client_sock.close()
        server_sock.close()
        print(f"[{conn_id}] Closed")


def main():
    parser = argparse.ArgumentParser(description="btest full MITM proxy")
    parser.add_argument("-t", "--target", required=True, help="Target MikroTik IP")
    parser.add_argument("-l", "--listen", type=int, default=2000, help="Listen port")
    args = parser.parse_args()

    listener = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    listener.bind(("0.0.0.0", args.listen))
    listener.listen(50)

    print(f"MITM proxy: 0.0.0.0:{args.listen} → {args.target}:2000")
    print(f"Point MikroTik btest client at this machine")
    print()

    conn_num = 0
    while True:
        client_sock, client_addr = listener.accept()
        client_sock.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
        conn_num += 1
        conn_id = f"TCP-{conn_num} {client_addr[0]}:{client_addr[1]}"
        print(f"\n{'='*60}")
        print(f"[{ts()}] New connection: {conn_id}")
        t = threading.Thread(
            target=proxy_tcp,
            args=(client_sock, args.target, 2000, conn_id),
            daemon=True,
        )
        t.start()


if __name__ == "__main__":
    main()
