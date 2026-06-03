"""Server-Sent Events broker.

Subscribers register and pull events; publishers call `publish(event_type, data)`.
The webapp's HTTP handler reads from a queue assigned at subscribe-time.

Event types: status_change, round_added, run_started, run_finished,
log_appended, activity_added, reporters_changed, system_operation.
"""
from __future__ import annotations

import json
import queue
import threading
import time
from typing import Any

_subscribers: list[queue.Queue] = []
_lock = threading.Lock()
_last_event_id = 0


def subscribe() -> queue.Queue:
    q: queue.Queue = queue.Queue(maxsize=256)
    with _lock:
        _subscribers.append(q)
    return q


def unsubscribe(q: queue.Queue) -> None:
    with _lock:
        try:
            _subscribers.remove(q)
        except ValueError:
            pass


def publish(event_type: str, data: Any) -> None:
    global _last_event_id
    with _lock:
        _last_event_id += 1
        evt_id = _last_event_id
        subs = list(_subscribers)
    payload = (evt_id, event_type, data)
    for q in subs:
        try:
            q.put_nowait(payload)
        except queue.Full:
            pass


def format_event(evt_id: int, event_type: str, data: Any) -> bytes:
    body = json.dumps(data, ensure_ascii=False)
    return f"id: {evt_id}\nevent: {event_type}\ndata: {body}\n\n".encode("utf-8")
