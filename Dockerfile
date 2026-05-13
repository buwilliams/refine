# refine-web — webapp container.
#
# Only the webapp runs in Docker. The host-side runner is started natively on
# the host so CLI subprocesses inherit ~/.claude auth, SSH keys, git config,
# and filesystem permissions.
#
# Source lives at /app and is on PYTHONPATH so docker-compose can bind-mount
# the static directory (or the whole refine_web package) over it without
# fighting site-packages layout.

FROM python:3.12-slim

WORKDIR /app

# stdlib-only at runtime. We don't pip install — just put the source on
# PYTHONPATH so bind mounts can transparently override files in place.
COPY refine/        ./refine/
COPY refine_shared/ ./refine_shared/
COPY refine_runner/ ./refine_runner/
COPY refine_web/    ./refine_web/

ENV PYTHONUNBUFFERED=1 \
    PYTHONDONTWRITEBYTECODE=1 \
    PYTHONPATH=/app

EXPOSE 8080

# WORKDIR is overridden by docker-compose to /refine-data so config discovery
# walks from the volume root.
ENTRYPOINT ["python", "-m", "refine", "web"]
