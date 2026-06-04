#!/usr/bin/env python3
"""
Webhook receiver for Cortex MCP — E2E testing.

Usage:
    python scripts/webhook_receiver.py [--port 9100] [--log file.jsonl]

Logs every received webhook to stdout (and optionally to a JSONL file).
Each line shows : timestamp, event, delivery_id, payload summary.

Use --quiet to suppress per-request logging (only summary at end).
"""

import argparse
import json
import sys
import time
from datetime import datetime, timezone
from http.server import BaseHTTPRequestHandler, HTTPServer
from threading import Lock


class WebhookCollector:
    """Thread-safe collector for received webhooks."""

    def __init__(self, log_path=None, quiet=False):
        self.received = []
        self.lock = Lock()
        self.log_path = log_path
        self.quiet = quiet
        self.start_time = time.time()

    def record(self, event, delivery_id, payload):
        entry = {
            "received_at": datetime.now(timezone.utc).isoformat(),
            "event": event,
            "delivery_id": delivery_id,
            "project_id": payload.get("project_id"),
            "data": payload.get("data", {}),
            "has_signature": "signature" in payload,
        }
        with self.lock:
            self.received.append(entry)
            if self.log_path:
                with open(self.log_path, "a") as f:
                    f.write(json.dumps(entry) + "\n")
        if not self.quiet:
            print(
                f"[{entry['received_at']}] event={event:20s} "
                f"delivery={delivery_id[:24]:24s} "
                f"project={entry['project_id'] or '-'} "
                f"sig={'yes' if entry['has_signature'] else 'no':3s} "
                f"data_keys={list(entry['data'].keys())}",
                flush=True,
            )

    def summary(self):
        elapsed = time.time() - self.start_time
        events = {}
        for entry in self.received:
            events[entry["event"]] = events.get(entry["event"], 0) + 1
        return {
            "total": len(self.received),
            "elapsed_seconds": round(elapsed, 2),
            "events_by_type": events,
        }


# Global collector (set by main())
collector = None


class Handler(BaseHTTPRequestHandler):
    def do_POST(self):
        length = int(self.headers.get("Content-Length", 0))
        body = self.rfile.read(length)
        event = self.headers.get("X-Cortex-Event", "?")
        delivery_id = self.headers.get("X-Cortex-Delivery-Id", "?")
        try:
            payload = json.loads(body)
        except json.JSONDecodeError:
            payload = {"_raw": body.decode("utf-8", errors="replace")}

        if collector:
            collector.record(event, delivery_id, payload)

        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.end_headers()
        self.wfile.write(b'{"status":"ok"}')

    def log_message(self, format, *args):
        # Silence the default access log
        pass


def main():
    p = argparse.ArgumentParser(description="Cortex webhook receiver")
    p.add_argument("--port", type=int, default=9100, help="listen port")
    p.add_argument("--log", type=str, default=None, help="JSONL log file")
    p.add_argument("--quiet", action="store_true", help="suppress per-request logging")
    args = p.parse_args()

    global collector
    collector = WebhookCollector(log_path=args.log, quiet=args.quiet)

    server = HTTPServer(("127.0.0.1", args.port), Handler)
    print(f"🛰  Cortex webhook receiver listening on http://127.0.0.1:{args.port}/", flush=True)
    print(f"   Log file: {args.log or '(stdout only)'}", flush=True)
    print(f"   Press Ctrl+C to stop and show summary.\n", flush=True)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        print("\n\n📊 Summary:", flush=True)
        print(json.dumps(collector.summary(), indent=2), flush=True)
        server.server_close()


if __name__ == "__main__":
    main()
