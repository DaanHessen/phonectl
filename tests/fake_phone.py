#!/usr/bin/env python3
"""A fake phonectl phone for testing the laptop daemon without a phone.

Speaks the link protocol (crates/phone/src/link.rs): HMAC handshake, then
sends a status, a clipboard clip and a notification, answers pings and calls
(type_text), records what the laptop sends, disconnects, and finally waits
for a UDP poke (the laptop's retry while the phone is away).

Prints one JSON object per observation on stdout. Stdlib only.

usage: fake_phone.py HOST PORT KEYFILE [--hold SECONDS] [--poke-wait SECONDS]
"""
import argparse
import base64
import hashlib
import hmac
import json
import os
import secrets
import select
import socket
import sys
import time

POKE_PORT = 47202


def proof(key, role, n1, n2):
    return hmac.new(key, f"phonectl-auth|{role}|{n1}|{n2}".encode(), hashlib.sha256).hexdigest()


def clip_hash(text):
    return hashlib.sha256(text.encode()).hexdigest()[:32]


def out(kind, **data):
    print(json.dumps({"kind": kind, **data}), flush=True)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("host")
    ap.add_argument("port", type=int)
    ap.add_argument("keyfile")
    ap.add_argument("--hold", type=float, default=20)
    ap.add_argument("--poke-wait", type=float, default=0)
    ap.add_argument("--clip", default=f"zz-phonectl-test-{os.getpid()} from the phone")
    a = ap.parse_args()
    key = base64.b64decode(open(a.keyfile).read().strip())

    s = socket.create_connection((a.host, a.port), timeout=10)
    pending = [b""]

    def send(obj):
        s.sendall((json.dumps(obj) + "\n").encode())

    def readline():
        while b"\n" not in pending[0]:
            chunk = s.recv(65536)
            if not chunk:
                raise EOFError("laptop closed the connection")
            pending[0] += chunk
        line, pending[0] = pending[0].split(b"\n", 1)
        return line

    n1 = secrets.token_hex(16)
    send({"type": "hello", "protocol": 1, "nonce": n1,
          "device": {"model": "FakePhone", "manufacturer": "Test", "android_version": "16", "sdk": 36},
          "tailscale_ip": a.host, "app_version": "test"})
    challenge = json.loads(readline())
    n2 = challenge["nonce"]
    ok = hmac.compare_digest(challenge["proof"], proof(key, "laptop", n1, n2))
    out("handshake", laptop_proof_ok=ok)
    if not ok:
        sys.exit(1)
    send({"type": "auth", "proof": proof(key, "phone", n1, n2)})
    ready = json.loads(readline())
    out("ready", name=ready.get("name"))

    now = int(time.time() * 1000)
    send({"type": "event", "topic": "status", "data": {
        "battery": {"level": 77, "charging": False},
        "network": {"type": "cellular", "cellular": "5G", "cellular_level": 3},
        "call": "idle",
        "permissions": {"notifications": True, "phone_state": True, "clipboard_auto": True,
                        "clipboard_mode": "keyboard", "keyboard": True, "bluetooth": True},
        "at": now}})
    send({"type": "event", "topic": "clipboard", "data": {"text": a.clip, "hash": clip_hash(a.clip), "at": now}})
    out("sent_clip", text=a.clip)
    send({"type": "event", "topic": "notification", "data": {
        "key": "0|org.example.chat|1|null|10001", "package": "org.example.chat", "app": "Chat",
        "title": "zz-phonectl-test", "text": "Hello from the fake phone", "posted_at": now,
        "clearable": True, "can_open": True, "actions": [{"index": 0, "title": "Mark as read"}]}})
    out("sent_notification")

    end = time.time() + a.hold
    buffer = pending[0]
    closed = False
    while time.time() < end and not closed:
        ready, _, _ = select.select([s], [], [], 1)
        if not ready:
            continue
        chunk = s.recv(65536)
        if not chunk:
            out("closed_by_laptop")
            break
        buffer += chunk
        while b"\n" in buffer:
            line, buffer = buffer.split(b"\n", 1)
            handle(json.loads(line), send)
    s.close()
    out("disconnected")
    return a, key


def handle(msg, send):
    t = msg.get("type")
    if t == "ping":
        send({"type": "pong", "id": msg.get("id")})
    elif t == "call":
        out("call", method=msg.get("method"), params=msg.get("params"))
        send({"type": "result", "id": msg["id"], "data": None})
    elif t == "event":
        data = msg.get("data") or {}
        if msg.get("topic") == "clipboard":
            out("laptop_clip", text=data.get("text"), hash_ok=data.get("hash") == clip_hash(data.get("text", "")))
        else:
            out("event", topic=msg.get("topic"))
    else:
        out("message", type=t)


def wait_for_poke(a, key):
    if a.poke_wait > 0:
        u = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        u.bind((a.host, POKE_PORT))
        u.settimeout(a.poke_wait)
        started = time.time()
        try:
            packet, _ = u.recvfrom(256)
            body, mac = packet[:-32], packet[-32:]
            valid = body.startswith(b"phonectl-poke") and hmac.compare_digest(mac, hmac.new(key, body, hashlib.sha256).digest())
            out("poke", after_s=round(time.time() - started), valid=valid)
        except (socket.timeout, TimeoutError):
            out("no_poke", waited_s=a.poke_wait)


if __name__ == "__main__":
    wait_for_poke(*main())
