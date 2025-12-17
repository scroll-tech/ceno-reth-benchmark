FROM nvidia/cuda:12.8.0-devel-ubuntu22.04

ARG RUNNER_VERSION="2.320.0"
ARG DEBIAN_FRONTEND=noninteractive

RUN apt update -y && apt upgrade -y && useradd -m docker
RUN apt install -y --no-install-recommends \
    curl jq build-essential libssl-dev libffi-dev python3 python3-venv python3-dev python3-pip \
    libkrb5-3 zlib1g libicu70 m4

RUN cd /home/docker && mkdir actions-runner && cd actions-runner \
    && curl -o actions-runner-linux-x64-2.320.0.tar.gz -L https://github.com/actions/runner/releases/download/v2.320.0/actions-runner-linux-x64-2.320.0.tar.gz \
    && echo "93ac1b7ce743ee85b5d386f5c1787385ef07b3d7c728ff66ce0d3813d5f46900  actions-runner-linux-x64-2.320.0.tar.gz" | shasum -a 256 -c \
    && tar xzf ./actions-runner-linux-x64-2.320.0.tar.gz

RUN chown -R docker ~docker && cd /home/docker/actions-runner/bin/ \
    && chmod +x installdependencies.sh \
    && ./installdependencies.sh

RUN apt install -y nvidia-utils-535
RUN apt install -y openssh-client
RUN apt install -y git

RUN apt install -y --no-install-recommends sudo && \
    echo "docker ALL=(ALL) NOPASSWD:ALL" >> /etc/sudoers

WORKDIR /home/docker/actions-runner

COPY ci/runner-entrypoint.sh /usr/local/bin/runner-entrypoint.sh
RUN chmod +x /usr/local/bin/runner-entrypoint.sh

ENTRYPOINT ["/usr/local/bin/runner-entrypoint.sh"]
