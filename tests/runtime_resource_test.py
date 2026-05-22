from pathlib import Path

from refine_runtime.backends.base import LaunchSpec
from refine_runtime.backends.systemd import SystemdBackend
from refine_runtime.resources import (
    ResourceSettings,
    detect_capabilities,
    validate_setting,
)


def main() -> int:
    assert validate_setting("worker_memory_limit_mb", "0") == "0"
    assert validate_setting("worker_memory_limit_mb", "4096") == "4096"
    try:
        validate_setting("worker_memory_limit_mb", "128")
        raise AssertionError("small memory limits should be rejected")
    except ValueError as e:
        assert "must be 0 or between 256 and 262144" in str(e)

    assert validate_setting("worker_cpu_priority", "VERY_LOW") == "very_low"
    assert validate_setting("resource_isolation_mode", "best_effort") == "best_effort"
    assert detect_capabilities("best_effort").name == "posix"

    settings = ResourceSettings(
        worker_memory_limit_mb=4096,
        ui_memory_limit_mb=1024,
        worker_cpu_priority="low",
    )
    agent_spec = LaunchSpec(
        args=["echo", "ok"],
        cwd=Path.cwd(),
        env={},
        kind="agent",
        settings=settings,
    )
    agent_cmd = SystemdBackend().command(agent_spec)
    assert "CPUWeight=50" in agent_cmd, agent_cmd
    assert "MemoryMax=4096M" in agent_cmd, agent_cmd

    ui_spec = LaunchSpec(
        args=["echo", "ok"],
        cwd=Path.cwd(),
        env={},
        kind="ui",
        settings=settings,
    )
    ui_cmd = SystemdBackend().command(ui_spec)
    assert "CPUWeight=100" in ui_cmd, ui_cmd
    assert "MemoryMax=1024M" in ui_cmd, ui_cmd

    print("runtime resource tests OK")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
