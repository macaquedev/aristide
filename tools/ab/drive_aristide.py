#!/usr/bin/env python3
"""Play tools/ab/passage.py live against a running aristide-server, via
its HTTP console API, in real wall-clock time -- the Aristide half of
the GO/Aristide A/B. Draws the registration once, then walks the event
list, sleeping to each event's absolute time before firing it.

Run this AFTER starting aristide-server with --record (see
tools/ab/README.md), and give the server a moment to finish loading the
set before starting this script -- it does not wait for readiness itself.

Usage: python3 tools/ab/drive_aristide.py [http://127.0.0.1:9901]
"""

import sys
import time
import urllib.request

from passage import REGISTRATION, build_passage


def post(base: str, path: str) -> None:
    req = urllib.request.Request(base + path, method="POST", data=b"")
    with urllib.request.urlopen(req, timeout=5) as resp:
        resp.read()


def main() -> None:
    base = sys.argv[1] if len(sys.argv) > 1 else "http://127.0.0.1:9901"

    print("drawing registration...")
    for stop_id, name, _manual in REGISTRATION:
        post(base, f"/api/stop?id={stop_id}&on=1")
        print(f"  drew [{stop_id}] {name}")

    events = build_passage()
    print(f"playing {len(events)} events over {events[-1].t:.1f}s...")

    t_start = time.monotonic()
    for e in events:
        target = t_start + e.t
        now = time.monotonic()
        if target > now:
            time.sleep(target - now)
        on = "1" if e.on else "0"
        post(base, f"/api/note?manual={e.manual}&key={e.key}&on={on}")

    print(f"done in {time.monotonic() - t_start:.2f}s wall time")


if __name__ == "__main__":
    main()
