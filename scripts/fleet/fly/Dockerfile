# Fleet worker image for Fly.io.
#
# The worker builds Refine from a Git ref chosen at provision time, so the
# control node's released binary can deploy workers running a newer Refine
# (REFINE_REF=main) or a pinned release (REFINE_REF=4.0.0). REFINE_REF accepts
# a branch, tag, or commit SHA.

FROM rust:1-bookworm AS builder

ARG REFINE_REPO_URL=https://github.com/buwilliams/refine.git
ARG REFINE_REF=main

WORKDIR /build
RUN git clone --depth 1 --branch "${REFINE_REF}" "${REFINE_REPO_URL}" refine \
    || (git clone "${REFINE_REPO_URL}" refine && cd refine && git checkout "${REFINE_REF}")

WORKDIR /build/refine
RUN cargo build --release --locked --bin refine

# The deployed marker makes the checkout behave exactly like a host install:
# ./r selects binary mode and system update can rebuild in place.
RUN mkdir -p bin \
    && install -m 755 target/release/refine bin/refine \
    && printf 'mode=deployed\nrelease_bin=bin/refine\nbuilt_at=%s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" > .refine-deployed

FROM debian:bookworm-slim

# Comma-separated agent provider CLIs to bake into the worker
# (claude, codex, gemini). Empty skips the install.
ARG AGENT_PROVIDERS=""

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl git openssh-client \
    && rm -rf /var/lib/apt/lists/*

RUN if [ -n "$AGENT_PROVIDERS" ]; then \
        apt-get update \
        && apt-get install -y --no-install-recommends nodejs npm \
        && rm -rf /var/lib/apt/lists/* \
        && for provider in $(printf '%s' "$AGENT_PROVIDERS" | tr ',' ' '); do \
            case "$provider" in \
                claude) npm install -g @anthropic-ai/claude-code ;; \
                codex) npm install -g @openai/codex ;; \
                gemini) npm install -g @google/gemini-cli ;; \
                *) echo "unknown agent provider: $provider" >&2; exit 1 ;; \
            esac; \
        done; \
    fi

WORKDIR /app
COPY --from=builder /build/refine /app
RUN rm -rf /app/target && ln -s /app/bin/refine /usr/local/bin/refine

# Workers are ephemeral: durable state lives in the target repo's .refine/
# directory and syncs through the shared Git remote, never on this machine.
EXPOSE 8080

# node init turns the machine into a working fleet member from injected
# secrets (REFINE_NODE_ID, REFINE_TARGET_REPO_URL, REFINE_AGENT_PROVIDERS,
# provider API key). Init failure still starts the daemon so operators can
# inspect and repair via `cluster run`.
CMD ["/bin/sh", "-c", "refine node init; exec refine system start --foreground --port 8080 --runtime-root run"]
