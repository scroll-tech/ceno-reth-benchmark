## Summary

Before building the docker image, fetch a fresh registration token from the GitHub [runner page](https://github.com/scroll-tech/ceno-reth-benchmark/settings/actions/runners/new?arch=x64&os=linux) and decide on a `SERVER_NAME` label for this runner.

Build the image (no secrets are needed at build time):
```shell
docker build -t ceno-reth-gpu:v1 .
```

Spin up the docker image by supplying the token and runner name at runtime:
```shell
docker run --gpus device=0 --name ceno-reth-runner -d \
  -e RUNNER_TOKEN="$TOKEN" \
  -e RUNNER_NAME="$SERVER_NAME" \
  -e RUNNER_URL="https://github.com/scroll-tech/ceno-reth-benchmark" \
  ceno-reth-gpu:v1
```

You may optionally set `RUNNER_LABELS` (defaults to `gpu`). The container’s entrypoint configures the runner via the provided token on startup and then launches `RUNNER_ALLOW_RUNASROOT=1 /home/docker/actions-runner/run.sh`, so no manual SSH is required.
