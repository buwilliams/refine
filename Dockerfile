# refine-web — webapp container.
#
# Only the webapp runs in Docker. The host-side runner (refine_runner) is
# started natively on the host so CLI subprocesses inherit ~/.claude auth,
# SSH keys, git config, and filesystem permissions.

FROM python:3.12-slim

# Build-time UID/GID let you align the in-container process owner with the host
# user that owns the bind-mounted volume root.
ARG REFINE_UID=1000
ARG REFINE_GID=1000

RUN groupadd --gid ${REFINE_GID} refine \
 && useradd --uid ${REFINE_UID} --gid ${REFINE_GID} --shell /bin/bash --create-home refine

WORKDIR /app

# stdlib-only Python — no requirements.txt needed.
COPY refine_shared/ ./refine_shared/
COPY refine_web/ ./refine_web/

RUN mkdir -p /var/run/refine \
 && chown -R refine:refine /app /var/run/refine

USER refine

EXPOSE 8080

ENV PYTHONUNBUFFERED=1 \
    PYTHONDONTWRITEBYTECODE=1 \
    REFINE_WEB_HOST=0.0.0.0 \
    REFINE_WEB_PORT=8080

ENTRYPOINT ["python", "-m", "refine_web"]
