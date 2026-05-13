# refine-web — webapp container.
#
# Only the webapp runs in Docker. The host-side runner is started natively on
# the host so CLI subprocesses inherit ~/.claude auth, SSH keys, git config,
# and filesystem permissions.

FROM python:3.12-slim

WORKDIR /app

# stdlib-only Python — no requirements.txt needed.
COPY refine/        ./refine/
COPY refine_shared/ ./refine_shared/
COPY refine_web/    ./refine_web/

ENV PYTHONUNBUFFERED=1 \
    PYTHONDONTWRITEBYTECODE=1

EXPOSE 8080

# WORKDIR is overridden by docker-compose to /refine-data so config discovery
# walks up from the volume root.
ENTRYPOINT ["python", "-m", "refine", "web"]
