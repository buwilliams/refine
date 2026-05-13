# refine-web — webapp container.
#
# Only the webapp runs in Docker. The host-side runner is started natively on
# the host so CLI subprocesses inherit ~/.claude auth, SSH keys, git config,
# and filesystem permissions.

FROM python:3.12-slim

WORKDIR /build

# stdlib-only at runtime. We use pip to install our own package so the
# `refine` console script lands on PATH.
COPY pyproject.toml ./
COPY refine/        ./refine/
COPY refine_shared/ ./refine_shared/
COPY refine_runner/ ./refine_runner/
COPY refine_web/    ./refine_web/

RUN pip install --no-cache-dir --root-user-action=ignore /build \
 && rm -rf /build

ENV PYTHONUNBUFFERED=1 \
    PYTHONDONTWRITEBYTECODE=1

EXPOSE 8080

# WORKDIR is overridden by docker-compose to /refine-data so config
# discovery walks from the volume root.
ENTRYPOINT ["refine", "web"]
