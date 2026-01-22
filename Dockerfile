# syntax=docker/dockerfile:1.6
FROM nvidia/cuda:12.8.1-devel-ubuntu24.04 AS builder

# System build deps
RUN apt-get update && apt-get install -y --no-install-recommends \
    build-essential \
    pkg-config \
    cmake \
    clang \
    libclang-dev \
    curl \
    openssh-client \
    git \
    m4 \
    ca-certificates \
  && rm -rf /var/lib/apt/lists/*

ARG GIT_HOST=github.com
# Pre-populate known_hosts so BuildKit's SSH mount only needs host key.
RUN mkdir -p /root/.ssh \
  && chmod 700 /root/.ssh \
  && ssh-keyscan -t rsa,ecdsa,ed25519 -H "${GIT_HOST}" >> /root/.ssh/known_hosts \
  && chmod 600 /root/.ssh/known_hosts

# Force cargo to use the CLI git implementation so forwarded SSH agent sockets
# are honored for private repositories.
ENV CARGO_NET_GIT_FETCH_WITH_CLI=true

RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
# Toolchains: stable for cargo-openvm, nightly for tco build
ENV CARGO_HOME="/root/.cargo" \
    RUSTUP_HOME="/root/.rustup" \
    PATH="/root/.cargo/bin:${PATH}"
RUN rustup toolchain install nightly-2025-11-20 \
  && rustup component add rust-src --toolchain nightly-2025-11-20 \
  && rustup default nightly-2025-11-20

RUN JEMALLOC_SYS_WITH_MALLOC_CONF="retain:true,metadata_thp:always,thp:always,dirty_decay_ms:-1,muzzy_decay_ms:-1,abort_conf:true" \
    cargo install --git https://github.com/scroll-tech/ceno.git --features jemalloc --features nightly-features cargo-ceno

WORKDIR /app
# Copy only Rust workspace files to keep build cache stable when server/ changes
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates/ ./crates/
COPY bin/ ./bin/
COPY rustfmt.toml ./

# Build guest ELF and place where host expects it
# --config net.git-fetch-with-cli=true for fetching private-repo
WORKDIR /app/bin/ceno-client-eth
RUN --mount=type=secret,id=sshkey \
    set -e; \
    KEY=/run/secrets/sshkey; \
    export GIT_SSH_COMMAND="ssh -i ${KEY} -o UserKnownHostsFile=/root/.ssh/known_hosts"; \
    cargo --config net.git-fetch-with-cli=true ceno build --release \
  && mkdir -p ../ceno-host/elf \
  && cp /app/bin/ceno-client-eth/target/riscv32im-ceno-zkvm-elf/release/ceno-client-eth ../ceno-host/elf/

# Build host binary
WORKDIR /app
ENV JEMALLOC_SYS_WITH_MALLOC_CONF="retain:true,background_thread:true,metadata_thp:always,dirty_decay_ms:10000,muzzy_decay_ms:10000,abort_conf:true"
ARG FEATURES="metrics,jemalloc,gpu"
ARG PROFILE="release"
ENV CUDA_ARCH="89"
RUN --mount=type=secret,id=sshkey \
    set -e; \
    KEY=/run/secrets/sshkey; \
    export GIT_SSH_COMMAND="ssh -i ${KEY} -o UserKnownHostsFile=/root/.ssh/known_hosts"; \
    cargo +nightly-2025-11-20 build --bin ceno-reth-benchmark-bin --profile=${PROFILE} --no-default-features --features=${FEATURES}

# Runtime image
FROM nvidia/cuda:12.8.1-runtime-ubuntu24.04 AS runtime
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates python3 python3-venv curl tar gzip \
   && rm -rf /var/lib/apt/lists/*
RUN S5CMD_VER=$(curl -s https://api.github.com/repos/peak/s5cmd/releases/latest | \
    grep tag_name | cut -d '"' -f 4) && \
    S5CMD_VER_TRIMMED=$(printf "%s" "$S5CMD_VER" | sed 's/^v//') && \
    curl -L -o /tmp/s5cmd.tar.gz "https://github.com/peak/s5cmd/releases/download/${S5CMD_VER}/s5cmd_${S5CMD_VER_TRIMMED}_Linux-64bit.tar.gz" && \
    tar xvf /tmp/s5cmd.tar.gz -C /usr/local/bin s5cmd && \
    rm /tmp/s5cmd.tar.gz

WORKDIR /app
COPY --from=builder /app/target/release/ceno-reth-benchmark-bin /usr/local/bin/ceno-reth-benchmark-bin
COPY --from=builder /app/bin/ceno-host/elf/ceno-client-eth /app/bin/ceno-host/elf/ceno-client-eth
COPY --from=builder /app/bin/ceno-client-eth/target/riscv32im-ceno-zkvm-elf/release/ceno-client-eth /app/target/riscv32im-ceno-zkvm-elf/release/ceno-client-eth
COPY --from=builder /app/bin/ceno-client-eth/target/riscv32im-ceno-zkvm-elf/release/ceno-client-eth /app/bin/ceno-client-eth/target/riscv32im-ceno-zkvm-elf/release/ceno-client-eth
COPY server /app/server

RUN python3 -m venv /opt/venv \
  && . /opt/venv/bin/activate \
  && pip install --no-cache-dir -r /app/server/requirements.txt

ENV RUST_LOG="info,p3_=warn" \
    OUTPUT_PATH="metrics.json" \
    JEMALLOC_SYS_WITH_MALLOC_CONF="retain:true,background_thread:true,metadata_thp:always,dirty_decay_ms:10000,muzzy_decay_ms:10000,abort_conf:true" \
    KZG_PARAMS_DIR="/root/.openvm/params" \
    CENO_GPU_CACHE_LEVEL="none"

# Useful mounts for cache/params
VOLUME ["/app/rpc-cache", "/root/.openvm/params"]

ENV PATH="/opt/venv/bin:${PATH}" \
    OVM_BIN="/usr/local/bin/ceno-reth-benchmark-bin" \
    WORKSPACE_ROOT="/app"

EXPOSE 8000
ENTRYPOINT ["uvicorn", "server.main:app", "--host", "0.0.0.0", "--port", "8000"]
