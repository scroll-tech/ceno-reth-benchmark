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

Use the host-side wrapper so an unrecoverable container GPU binding is replaced
with a fresh container automatically:

```bash
./server/run_container.sh --gpus all \
  -p 8000:8000 \
  -v /path/on/host/jobs:/app/jobs \
  -e CENO_STATUS_API_BASE_URL="https://staging--ethproofs.netlify.app/api/v0" \
  -e CENO_STATUS_API_KEY="<api-token>" \
  -e CENO_CLUSTER_ID="<cluster-id>" \
  -e ETH_RPC_URL="<RPC URL>" \
  reth-server:latest
```

The server leaves `CENO_CHIP_PROVING_MODE` and `CENO_CHIP_PROVING_LANES`
unset by default, so the pinned Ceno revision owns the scheduler defaults.
Set either variable only for an explicit override. The removed
`CENO_CONCURRENT_CHIP_PROVING` setting is not supported.

If the host GPU goes offline and later recovers, an existing container can keep
stale NVIDIA device bindings and report `Failed to initialize NVML: Unknown
Error`. The server polls briefly for a transient recovery, then exits with status
75. `run_container.sh` removes that container and creates a fresh one after 10
seconds. Override the container name or delay with `CONTAINER_NAME` and
`GPU_RECREATE_DELAY_SEC`. If host `nvidia-smi` is available, the wrapper waits
for it to recover before creating the replacement; configure that polling with
`HOST_GPU_POLL_INTERVAL_SEC`.

```bash
CONTAINER_NAME=reth-server GPU_RECREATE_DELAY_SEC=30 \
  ./server/run_container.sh --gpus all ... reth-server:latest
```

The wrapper must run on the Docker host. Mounting the Docker socket into the
container is intentionally not required.

Mounting `/app/jobs` persists `block_data` and logs between runs. Set `CENO_STATUS_API_BASE_URL`, `CENO_STATUS_API_KEY`, and `CENO_CLUSTER_ID` to report queue/proving/proved events to the API (omit them to skip the HTTP hooks). Configure any other env vars (APP_PK_URI, AGG_PK_URI, JOBS_DIR, etc.) as needed.

To debug a specific block instead of the latest, append
`-e BLOCK_NUMBER="<BLOCKNUM>"` to the wrapper command.
