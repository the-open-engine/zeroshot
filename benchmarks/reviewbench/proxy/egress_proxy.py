#!/usr/bin/env python3
"""Forward proxy that admits HTTPS CONNECT only to allowlisted hosts.

Reviewer containers sit on an internal Docker network whose only way out is
this proxy, so the allowlist (model provider hosts) is the whole network the
reviewer can reach. Every request is logged as one JSON line.

    egress_proxy.py --listen 0.0.0.0:3128 --allow api.anthropic.com --log /log/egress.jsonl
"""

from __future__ import annotations

import argparse
import asyncio
import json
import time

ALLOW: set[str] = set()
LOG = None


def log(**event) -> None:
    event["ts"] = round(time.time(), 3)
    LOG.write(json.dumps(event, sort_keys=True) + "\n")
    LOG.flush()


def allowed(host: str, port: int) -> bool:
    return port == 443 and host.lower().rstrip(".") in ALLOW


async def pipe(reader: asyncio.StreamReader, writer: asyncio.StreamWriter) -> int:
    total = 0
    try:
        while data := await reader.read(65536):
            total += len(data)
            writer.write(data)
            await writer.drain()
    except (ConnectionError, asyncio.CancelledError):
        pass
    finally:
        try:
            writer.close()
        except Exception:
            pass
    return total


async def handle(client_reader: asyncio.StreamReader, client_writer: asyncio.StreamWriter) -> None:
    peer = client_writer.get_extra_info("peername")
    client = peer[0] if peer else "?"
    try:
        head = await asyncio.wait_for(client_reader.readuntil(b"\r\n\r\n"), timeout=30)
    except Exception:
        client_writer.close()
        return
    request_line = head.split(b"\r\n", 1)[0].decode("latin-1")
    parts = request_line.split()
    if len(parts) < 2:
        client_writer.close()
        return
    method, target = parts[0].upper(), parts[1]
    if method != "CONNECT":
        log(client=client, method=method, target=target[:200], decision="deny")
        client_writer.write(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
        await client_writer.drain()
        client_writer.close()
        return
    host, _, port_text = target.rpartition(":")
    host = host.strip("[]")
    port = int(port_text) if port_text.isdigit() else 0
    if not allowed(host, port):
        log(client=client, method=method, host=host, port=port, decision="deny")
        client_writer.write(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
        await client_writer.drain()
        client_writer.close()
        return
    try:
        upstream_reader, upstream_writer = await asyncio.wait_for(asyncio.open_connection(host, port), timeout=30)
    except Exception as error:
        log(client=client, method=method, host=host, port=port, decision="allow", error=str(error)[:200])
        client_writer.write(b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
        await client_writer.drain()
        client_writer.close()
        return
    client_writer.write(b"HTTP/1.1 200 Connection Established\r\n\r\n")
    await client_writer.drain()
    started = time.time()
    sent, received = await asyncio.gather(
        pipe(client_reader, upstream_writer), pipe(upstream_reader, client_writer)
    )
    log(client=client, method=method, host=host, port=port, decision="allow",
        sent=sent, received=received, seconds=round(time.time() - started, 1))


async def main() -> None:
    global LOG
    parser = argparse.ArgumentParser()
    parser.add_argument("--listen", default="0.0.0.0:3128")
    parser.add_argument("--allow", action="append", default=[])
    parser.add_argument("--log", required=True)
    args = parser.parse_args()
    ALLOW.update(h.lower() for h in args.allow)
    LOG = open(args.log, "a", buffering=1)
    host, _, port = args.listen.rpartition(":")
    server = await asyncio.start_server(handle, host, int(port), limit=1 << 20)
    log(event="start", allow=sorted(ALLOW), listen=args.listen)
    async with server:
        await server.serve_forever()


if __name__ == "__main__":
    asyncio.run(main())
