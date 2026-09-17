# Self-hosted GPU Actions runner

This image is a GitHub Actions runner with the CUDA and build toolchain needed by
`Ceno Benchmark v2`. It is not the benchmark server image. Build
`ci/Dockerfile`; do not use the repository-root Dockerfile or
`reth-server:latest`. The workflow checks out and builds the requested benchmark
and Ceno revisions after the runner accepts a job.

## 1. Build the runner image on the CI host

The runner Dockerfile uses a BuildKit SSH mount to check access to private Git
dependencies. It does not consume the root Dockerfile's `--secret`, `FEATURES`,
or `GIT_HOST` build arguments.

```bash
cd /path/to/ceno-reth-benchmark

DOCKER_BUILDKIT=1 docker build -f ci/Dockerfile \
  --ssh sshkey=$HOME/.ssh/id_rsa \
  -t ceno-reth-gpu-runner:latest .
```

## 2. Register one runner container with two GPUs

Create a fresh self-hosted runner registration token at
<https://github.com/scroll-tech/ceno-reth-benchmark/settings/actions/runners/new?arch=x64&os=linux>.
`RUNNER_TOKEN` is this short-lived, repository-specific registration token, not
a general GitHub personal access token (PAT). It is used by `config.sh` to
register the container. A separately authenticated `gh` CLI or PAT may be used
to inspect and dispatch workflows.

```bash
RUNNER_TOKEN="<FRESH_RUNNER_REGISTRATION_TOKEN>"
RUNNER_NAME="<DUAL_GPU_RUNNER_NAME>"

docker run -d \
  --restart unless-stopped \
  --gpus '"device=0,1"' \
  --name ceno-reth-runner-dual \
  -e RUNNER_TOKEN="$RUNNER_TOKEN" \
  -e RUNNER_NAME="$RUNNER_NAME" \
  -e RUNNER_URL="https://github.com/scroll-tech/ceno-reth-benchmark" \
  -e RUNNER_LABELS="dual-gpu" \
  ceno-reth-gpu-runner:latest
```

Both GPUs must be exposed to the same container. Docker presents them to the
benchmark as logical CUDA devices `0` and `1`, regardless of their host
ordinals. Confirm the device view and wait for the runner to report `online`:

```bash
docker exec ceno-reth-runner-dual nvidia-smi -L

gh api repos/scroll-tech/ceno-reth-benchmark/actions/runners \
  --jq '.runners[] | [.name, .status] | @tsv' \
  | grep -F "${RUNNER_NAME}"$'\tonline'
```

The second command requires `gh auth` with repository administration access.
The runner also appears under **Settings → Actions → Runners** with the
`dual-gpu` label.

## 3. Dispatch `Ceno Benchmark v2`

GitHub Actions can only check out refs and commits that have been pushed to a
repository it can access. Replace both placeholders below with pushed refs. In
particular, local campaign commits are not visible to GitHub until someone with
the appropriate authority pushes them; this setup does not push anything.

```bash
BENCHMARK_BRANCH="<BENCHMARK_BRANCH>"
CENO_VERSION="<CENO_BRANCH_OR_SHA>"

gh workflow run run-benchmark-v2.yml \
  --repo scroll-tech/ceno-reth-benchmark \
  --ref "$BENCHMARK_BRANCH" \
  -f ceno_version="$CENO_VERSION" \
  -f gpu_runner_label=dual-gpu \
  -f gpu_devices=0,1 \
  -f block_number=23587691 \
  -f chain_id=1 \
  -f max_cell_per_shard=2684354560 \
  -f run_gpu_benchmark=true \
  -f run_gpu_mem_estimation=false
```

Using `chain_id=1` keeps the benchmark on its generated/cached input path and
avoids putting a raw RPC URL in commands or logs. The workflow defaults remain
`gpu_runner_label=gpu` and `gpu_devices=0` for an existing one-GPU runner.

## 4. Watch the run and save its raw log

List recent workflow runs, copy the run ID for the dispatch above, then watch it
and save the complete job log locally for debugging:

```bash
gh run list \
  --repo scroll-tech/ceno-reth-benchmark \
  --workflow run-benchmark-v2.yml \
  --branch "$BENCHMARK_BRANCH" \
  --limit 10

RUN_ID="<GITHUB_ACTIONS_RUN_ID>"

gh run watch "$RUN_ID" \
  --repo scroll-tech/ceno-reth-benchmark \
  --exit-status

gh run view "$RUN_ID" \
  --repo scroll-tech/ceno-reth-benchmark \
  --log > "ceno-benchmark-${RUN_ID}.log"
```

The downloaded log is the evidence to inspect for selected devices, replay
ownership, proof collection, verification, and failures. Merely configuring the
runner does not constitute physical two-GPU validation.
