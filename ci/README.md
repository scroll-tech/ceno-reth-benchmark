## Summary

Before building the docker image, we need to fetch the token from GitHub [page](https://github.com/scroll-tech/ceno-reth-benchmark/settings/actions/runners/new?arch=x64&os=linux). The docker image for running our own self-hosted runner can be built from
```shell
docker build -t ceno-reth-gpu:v1 .
```

And then we can spin up the docker image via
```shell
docker run --gpus device=0 -it ceno-reth-gpu:v1 /bin/bash
```
