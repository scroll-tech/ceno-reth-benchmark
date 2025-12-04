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

- GPU build (default): `--build-arg FEATURES="metrics,jemalloc,gpu"`
- CPU-only build: `--build-arg FEATURES="metrics,jemalloc"` (omit GPU extras)

### Run

```bash
docker run --gpus all \
  -p 8000:8000 \
  -v /path/on/host/jobs:/app/jobs \
  -e CENO_STATUS_API_BASE_URL="https://staging--ethproofs.netlify.app/api/v0" \
  -e CENO_STATUS_API_KEY="<api-token>" \
  -e CENO_CLUSTER_ID="<cluster-id>" \
  -e ETH_RPC_URL="<RPC URL>" \
  reth-server:latest
```

Mounting `/app/jobs` persists `block_data` and logs between runs. Set `CENO_STATUS_API_BASE_URL`, `CENO_STATUS_API_KEY`, and `CENO_CLUSTER_ID` to report queue/proving/proved events to the API (omit them to skip the HTTP hooks). Configure any other env vars (APP_PK_URI, AGG_PK_URI, JOBS_DIR, etc.) as needed.

To debug a specific block instead of the latest, append `-e BLOCK_NUMBER="<BLOCKNUM>"` to the `docker run` command.
