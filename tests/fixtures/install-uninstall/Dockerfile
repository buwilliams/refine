FROM rust:1-bookworm

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        bash \
        ca-certificates \
        curl \
        git \
        netcat-openbsd \
        procps \
    && rm -rf /var/lib/apt/lists/* \
    && printf '%s\n' 'export PATH=/usr/local/cargo/bin:/home/refine/.cargo/bin:$PATH' >/etc/profile.d/refine-cargo-path.sh \
    && useradd -m -s /bin/bash refine

USER refine
WORKDIR /home/refine

ENV CARGO_HOME=/home/refine/.cargo
ENV PATH=/usr/local/cargo/bin:/home/refine/.cargo/bin:${PATH}

CMD ["sleep", "infinity"]
