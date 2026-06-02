"""Internal supervisor IPC method names.

The supervisor is the only local process authority. UI and worker code use
these Unix-socket methods to request worker lifecycle and managed subprocess
operations instead of launching or killing OS processes directly.
"""
from __future__ import annotations


M_STATUS = "status"
M_SHUTDOWN = "shutdown"
M_SWITCH_APP = "switch_app"
M_DETACH_APP = "detach_app"
M_ENSURE_WORKER = "ensure_worker"
M_STOP_WORKER = "stop_worker"
M_TARGET_APP_RUN = "target_app_run"
M_PROCESS_LAUNCH = "process_launch"
M_PROCESS_WRITE = "process_write"
M_PROCESS_READ = "process_read"
M_PROCESS_SIGNAL = "process_signal"
M_PROCESS_WAIT = "process_wait"

WORKER_STARTUP_TIMEOUT_SECONDS = 60.0


PROCESS_METHODS = {
    M_PROCESS_LAUNCH,
    M_PROCESS_WRITE,
    M_PROCESS_READ,
    M_PROCESS_SIGNAL,
    M_PROCESS_WAIT,
}
