## How to build and run

At the repo root (so `cd ..` first)


### Build variants

All builds require access to the private [`ceno-gpu`](https://github.com/scroll-tech/ceno-gpu/) repo, so forward SSH key:

```bash
DOCKER_BUILDKIT=1 docker build \
  --secret id=sshkey,src=$HOME/.ssh/<PRI_KEY_FILE_PATH> \
  --build-arg GIT_HOST=github.com \
  -t reth-server:latest \
  .
```

Select features via `--build-arg FEATURES=...`:

- GPU build (default): `--build-arg FEATURES="metrics,jemalloc,gpu,aot,parallel"`
- CPU-only build: `--build-arg FEATURES="metrics,jemalloc"` (omit GPU extras)

### Run

The image is independent of the number of GPUs. At container startup, Docker
chooses which physical GPUs are visible and `CENO_GPU_DEVICES` chooses which
container-local logical GPU ordinals Ceno uses. It defaults to logical GPU `0`.

Single-GPU proving with only host GPU 0 visible:

```bash
server/run_container.sh --gpus '"device=0"' \
  -p 8000:8000 \
  -v /path/on/host/jobs:/app/jobs \
  -e CENO_STATUS_API_BASE_URL="https://staging--ethproofs.netlify.app/api/v0" \
  -e CENO_STATUS_API_KEY="<api-token>" \
  -e CENO_CLUSTER_ID="<cluster-id>" \
  -e ETH_RPC_URL="<RPC URL>" \
  -e CENO_GPU_DEVICES="0" \
  reth-server:latest
```

Dual-GPU proving with host GPUs 0 and 1 visible:

```bash
server/run_container.sh --gpus '"device=0,1"' \
  -p 8000:8000 \
  -v /path/on/host/jobs:/app/jobs \
  -e CENO_STATUS_API_BASE_URL="https://staging--ethproofs.netlify.app/api/v0" \
  -e CENO_STATUS_API_KEY="<api-token>" \
  -e CENO_CLUSTER_ID="<cluster-id>" \
  -e ETH_RPC_URL="<RPC URL>" \
  -e CENO_GPU_DEVICES="0,1" \
  reth-server:latest
```

The same pattern supports more GPUs. For example, expose four host GPUs and
select all four logical devices:

```bash
server/run_container.sh --gpus '"device=0,1,2,3"' \
  -p 8000:8000 \
  -v /path/on/host/jobs:/app/jobs \
  -e ETH_RPC_URL="<RPC URL>" \
  -e CENO_GPU_DEVICES="0,1,2,3" \
  reth-server:latest
```

It is also valid to expose several GPUs but use only a subset. For example,
with host GPUs 0 and 1 exposed, `CENO_GPU_DEVICES="0"` runs single-GPU proving
and `CENO_GPU_DEVICES="0,1"` runs dual-GPU proving. Device IDs are logical
inside the container, not necessarily the host's original ordinals.

The startup log prints the effective list as `Ceno logical GPU devices`, and
the prover log prints `ceno multi-gpu devices`. Invalid, duplicate, or
unavailable device IDs fail before proving.

The server leaves `CENO_CHIP_PROVING_MODE` and `CENO_CHIP_PROVING_LANES`
unset by default, so the pinned Ceno revision owns the scheduler defaults.
Set either variable only for an explicit override. The removed
`CENO_CONCURRENT_CHIP_PROVING` setting is not supported.

Run these commands on the host in the foreground, under your host service
manager if needed. `server/run_container.sh` owns the container name (default
`reth-server`; override with `CONTAINER_NAME`) and lifecycle. Do not pass Docker's
`-d`, `--name`, or `--restart` options to this wrapper.

If the GPU goes offline, the watchdog detects CUDA/NVML failures in job stderr,
including output written before a new log is discovered. The entrypoint gives
prover processes up to `STACK_STOP_TIMEOUT_SEC` (default 10 seconds) to stop,
then kills them and exits with status 75. The host wrapper removes the old
container, waits for host GPU availability, and creates a new container with
the same arguments and job volume. Other container exit codes are returned to
the host service manager. Historical job errors are ignored on startup.

Use the host wrapper for automatic recovery: plain `docker run` does not
recreate a container on status 75. Restarting only the prover inside a container
cannot repair stale NVIDIA device bindings. Host GPU checks require
`nvidia-smi` and GNU `timeout`; without host `nvidia-smi`, the container's startup
GPU check handles readiness.

Mounting `/app/jobs` persists `block_data` and logs between runs. Set `CENO_STATUS_API_BASE_URL`, `CENO_STATUS_API_KEY`, and `CENO_CLUSTER_ID` to report queue/proving/proved events to the API (omit them to skip the HTTP hooks). Configure any other env vars (APP_PK_URI, AGG_PK_URI, JOBS_DIR, etc.) as needed.

The server serializes the GPU proving section across proof UUIDs. Each prover
sizes its pools under the assumption that it owns all selected devices; running
two prover processes against the same GPUs concurrently is unsupported. Job logs
include the locked Ceno and ceno-gpu revisions plus host, guest, and
`Cargo.lock` hashes so a server failure can be compared exactly with CI.

To debug a specific block instead of the latest, append `-e BLOCK_NUMBER="<BLOCKNUM>"` to the `server/run_container.sh` command (before the image name).
