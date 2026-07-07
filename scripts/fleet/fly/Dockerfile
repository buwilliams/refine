# Fleet worker image for Fly.io.
#
# The worker builds Refine from a Git ref chosen at provision time, so the
# control node's released binary can deploy workers running a newer Refine
# (REFINE_REF=main) or a pinned release (REFINE_REF=3.1.6). REFINE_REF accepts
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

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl git openssh-client \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY --from=builder /build/refine /app
RUN rm -rf /app/target && ln -s /app/bin/refine /usr/local/bin/refine

# Workers are ephemeral: durable state lives in the target repo's .refine/
# directory and syncs through the shared Git remote, never on this machine.
EXPOSE 8080

CMD ["refine", "system", "start", "--foreground", "--port", "8080", "--runtime-root", "run"]
