# syntax=docker/dockerfile:1.7

FROM nvidia/cuda:12.8.0-devel-ubuntu22.04

ARG RUNNER_VERSION="2.320.0"
ARG DEBIAN_FRONTEND=noninteractive

RUN apt-get update \
    && apt-get upgrade -y \
    && apt-get install -y --no-install-recommends software-properties-common \
    && add-apt-repository -y ppa:deadsnakes/ppa \
    && apt-get update \
    && apt-get install -y --no-install-recommends \
        curl jq build-essential cmake libssl-dev libffi-dev python3.11 python3.11-venv python3.11-dev python3-pip \
        libkrb5-3 zlib1g libicu70 m4 \
    && rm -rf /var/lib/apt/lists/* \
    && useradd -m docker
RUN update-alternatives --install /usr/bin/python3 python3 /usr/bin/python3.11 1

RUN cd /home/docker && mkdir actions-runner && cd actions-runner \
    && curl -o actions-runner-linux-x64-2.320.0.tar.gz -L https://github.com/actions/runner/releases/download/v2.320.0/actions-runner-linux-x64-2.320.0.tar.gz \
    && echo "93ac1b7ce743ee85b5d386f5c1787385ef07b3d7c728ff66ce0d3813d5f46900  actions-runner-linux-x64-2.320.0.tar.gz" | shasum -a 256 -c \
    && tar xzf ./actions-runner-linux-x64-2.320.0.tar.gz

RUN chown -R docker ~docker && cd /home/docker/actions-runner/bin/ \
    && chmod +x installdependencies.sh \
    && ./installdependencies.sh

RUN apt-get update \
    && apt-get install -y --no-install-recommends nvidia-utils-535 openssh-client git \
    && rm -rf /var/lib/apt/lists/*

# Validate SSH access for private dependencies (requires --ssh sshkey=... at build time)
RUN --mount=type=ssh,id=sshkey \
    set -eux; \
    mkdir -p /root/.ssh; \
    chmod 700 /root/.ssh; \
    ssh-keyscan github.com >> /root/.ssh/known_hosts; \
    git ls-remote git@github.com:scroll-tech/ceno-reth-benchmark.git >/dev/null

RUN apt-get update && apt-get install -y --no-install-recommends sudo && \
    rm -rf /var/lib/apt/lists/* && \
    echo "docker ALL=(ALL) NOPASSWD:ALL" >> /etc/sudoers

WORKDIR /home/docker/actions-runner

COPY ci/runner-entrypoint.sh /usr/local/bin/runner-entrypoint.sh
RUN chmod +x /usr/local/bin/runner-entrypoint.sh

ENTRYPOINT ["/usr/local/bin/runner-entrypoint.sh"]
