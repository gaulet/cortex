#!/usr/bin/env python3
"""
E2E test for Cortex webhooks.

Orchestrates:
1. A Python HTTP receiver on port 9100
2. A cortex-mcp binary as subprocess, with CORTEX_WEBHOOK_URLS pointing
   to the receiver
3. Sends MCP requests to cortex via stdin pipe
4. Waits for the receiver to log the corresponding webhooks
5. Verifies the expected events were received

This is the real E2E test — same binary, same flow as in production.

Usage:
    python scripts/e2e_webhook_test.py [--binary path/to/cortex-mcp]
"""

import argparse
import json
import os
import subprocess
import sys
import threading
import time
from collections import defaultdict
from http.server import BaseHTTPRequestHandler, HTTPServer


# --- Receiver ---------------------------------------------------------------

class Collector:
    def __init__(self):
        self.events = []
        self.lock = threading.Lock()

    def add(self, event, delivery_id, payload):
        with self.lock:
            self.events.append({
                "received_at": time.time(),
                "event": event,
                "delivery_id": delivery_id,
                "project_id": payload.get("project_id"),
                "data_keys": sorted(list(payload.get("data", {}).keys())),
                "has_signature": "signature" in payload,
            })

    def count_by_event(self):
        with self.lock:
            counts = defaultdict(int)
            for e in self.events:
                counts[e["event"]] += 1
            return dict(counts)


class Handler(BaseHTTPRequestHandler):
    collector = None  # set before server starts

    def do_POST(self):
        length = int(self.headers.get("Content-Length", 0))
        body = self.rfile.read(length)
        event = self.headers.get("X-Cortex-Event", "?")
        delivery_id = self.headers.get("X-Cortex-Delivery-Id", "?")
        try:
            payload = json.loads(body)
        except json.JSONDecodeError:
            payload = {}
        if Handler.collector:
            Handler.collector.add(event, delivery_id, payload)
        self.send_response(200)
        self.end_headers()
        self.wfile.write(b'{"status":"ok"}')

    def log_message(self, *a, **kw):
        pass


def start_receiver(port=9100):
    collector = Collector()
    Handler.collector = collector
    server = HTTPServer(("127.0.0.1", port), Handler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    return collector, server


# --- MCP client (talks to cortex via stdin) ---------------------------------

class CortexStdioClient:
    def __init__(self, binary_path, env_extra=None):
        env = os.environ.copy()
        if env_extra:
            env.update(env_extra)
        env["CORTEX_WEBHOOK_URLS"] = f"http://127.0.0.1:9100/"
        env["CORTEX_WEBHOOK_TIMEOUT"] = "5"
        env["CORTEX_WEBHOOK_MAX_RETRIES"] = "2"
        self.proc = subprocess.Popen(
            [binary_path],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            env=env,
            bufsize=0,
        )
        self._next_id = 1

    def call(self, tool, args):
        req = {
            "jsonrpc": "2.0",
            "method": "tools/call",
            "params": {"name": tool, "arguments": args},
            "id": self._next_id,
        }
        self._next_id += 1
        line = json.dumps(req) + "\n"
        self.proc.stdin.write(line.encode())
        self.proc.stdin.flush()
        # Read the response (one line of JSON)
        response = self.proc.stdout.readline().decode()
        return json.loads(response) if response.strip() else None

    def close(self):
        try:
            self.proc.stdin.close()
        except Exception:
            pass
        # Give the async tasks a moment to complete
        time.sleep(2)
        try:
            self.proc.terminate()
            self.proc.wait(timeout=3)
        except Exception:
            self.proc.kill()


# --- Test scenarios ---------------------------------------------------------

def run_scenarios(client):
    print("  → intercept_plan (should fire plan_generated)")
    r = client.call("intercept_plan", {
        "intent": "Refactor authentication",
        "context": "FastAPI JWT-based",
        "project_id": "e2e-A",
    })
    assert r and "result" in r, f"intercept_plan failed: {r}"
    plan_text = r["result"]["content"][0]["text"]
    plan = json.loads(plan_text)
    print(f"    project_id=e2e-A, themes={len(plan['plan']['themes'])}")

    # NOTE: pre_mortem and sync_reflect are skipped because the MockLlmClient
    # returns a plan-shaped response for every call, which doesn't satisfy
    # the schemas of other tools. In production with a real LLM, those
    # webhooks would fire here.

    print("  → abort (should fire abort)")
    r = client.call("abort", {
        "project_id": "e2e-B",
        "reason": "E2E test abort",
    })
    assert r and "result" in r, f"abort failed: {r}"
    print("    aborted_at logged")


def main():
    p = argparse.ArgumentParser(description="Cortex webhook E2E test")
    p.add_argument("--binary", default="./target/release/cortex-mcp.exe",
                   help="path to cortex-mcp binary")
    p.add_argument("--port", type=int, default=9100, help="receiver port")
    args = p.parse_args()

    if not os.path.isfile(args.binary):
        print(f"❌ Binary not found: {args.binary}", file=sys.stderr)
        print("   Build with: cargo build --release -p cortex-mcp-server", file=sys.stderr)
        sys.exit(1)

    print(f"🛰  Starting receiver on port {args.port}...")
    collector, server = start_receiver(args.port)
    time.sleep(0.3)
    print(f"   Receiver ready.")

    print(f"🚀 Spawning cortex binary: {args.binary}")
    client = CortexStdioClient(args.binary)
    print(f"   PID: {client.proc.pid}")
    # Stream stderr in a background thread
    import threading
    def _drain_stderr():
        while True:
            line = client.proc.stderr.readline()
            if not line:
                return
            print(f"   [cortex] {line.decode().rstrip()}", flush=True)
    threading.Thread(target=_drain_stderr, daemon=True).start()

    try:
        print("\n📞 Running scenarios...")
        run_scenarios(client)
        # CRUCIAL: leave the binary alive for a few seconds so the async
        # webhook tasks can complete. fire() is fire-and-forget via
        # tokio::spawn — if we close stdin right now, the runtime drops
        # and the in-flight tasks are killed.
        print("\n⏳ Waiting 3s for async webhooks to deliver...")
        time.sleep(3)
    finally:
        print("\n🛑 Shutting down cortex...")
        client.close()
        time.sleep(1)
        server.shutdown()
        server.server_close()

    # --- Verify ---
    print("\n📊 Verification:")
    counts = collector.count_by_event()
    print(f"   Total webhooks received: {sum(counts.values())}")
    for event, count in sorted(counts.items()):
        print(f"     - {event:20s} × {count}")

    expected = {
        "plan_generated": 1,      # 1 intercept_plan call
        "abort": 1,               # 1 abort call
        # NOT expected: pre_mortem_emitted, audit_failed (skipped — mock LLM)
        # NOT expected: recovery_triggered (no crash)
    }
    print("\n✅ Expected events:")
    for event, count in sorted(expected.items()):
        actual = counts.get(event, 0)
        mark = "✅" if actual == count else "❌"
        print(f"     {mark} {event:20s} expected={count} got={actual}")

    unexpected_present = set(counts.keys()) - set(expected.keys())
    if unexpected_present:
        print(f"\n⚠️  Unexpected events: {unexpected_present}")

    all_good = all(counts.get(e, 0) == n for e, n in expected.items())
    sys.exit(0 if all_good else 1)


if __name__ == "__main__":
    main()
