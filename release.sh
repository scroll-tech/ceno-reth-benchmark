#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage: release.sh <tag> [release-name]

Builds the ceno-reth-benchmark-bin with cargo and uploads the artifact
to the scroll-tech/ceno-reth-benchmark GitHub releases using the gh CLI.

Environment variables:
  GITHUB_REPO        Override the repo to publish to (default: scroll-tech/ceno-reth-benchmark)
  RELEASE_NOTES      Release notes string (ignored if RELEASE_NOTES_FILE is set)
  RELEASE_NOTES_FILE Path to a file that will be used for --notes-file
  ARTIFACT_DIR       Directory to store the packaged binary (default: release-artifacts)
EOF
}

if [[ $# -lt 1 ]]; then
  usage >&2
  exit 2
fi

if ! command -v gh >/dev/null 2>&1; then
  echo "[release.sh] gh CLI not found. Install GitHub CLI: https://cli.github.com/" >&2
  exit 1
fi

if ! command -v cargo >/dev/null 2>&1; then
  echo "[release.sh] cargo not found in PATH" >&2
  exit 1
fi

TAG="$1"
RELEASE_NAME="${2:-$TAG}"
REPO="${GITHUB_REPO:-scroll-tech/ceno-reth-benchmark}"
BINARY_NAME="ceno-reth-benchmark-bin"
ARTIFACT_DIR="${ARTIFACT_DIR:-release-artifacts}"
PROFILE="release"
BUILD_PATH="target/${PROFILE}/${BINARY_NAME}"

echo "[release.sh] Building ${BINARY_NAME} (${PROFILE})"
RUSTFLAGS="-C target-feature=+avx2" JEMALLOC_SYS_WITH_MALLOC_CONF="retain:true,metadata_thp:always,thp:always,dirty_decay_ms:-1,muzzy_decay_ms:-1" cargo build --features jemalloc --features metrics --features gpu --locked --release --bin "${BINARY_NAME}"

if [[ ! -f "${BUILD_PATH}" ]]; then
  echo "[release.sh] Build succeeded but ${BUILD_PATH} not found" >&2
  exit 1
fi

mkdir -p "${ARTIFACT_DIR}"
OS="$(uname -s | tr '[:upper:]' '[:lower:]')"
ARCH="$(uname -m)"
ARTIFACT_STEM="${BINARY_NAME}-${TAG}-${OS}-${ARCH}"
TMP_DIR="$(mktemp -d)"
trap 'rm -rf "${TMP_DIR}"' EXIT

cp "${BUILD_PATH}" "${TMP_DIR}/${ARTIFACT_STEM}"
chmod +x "${TMP_DIR}/${ARTIFACT_STEM}"
ARCHIVE_PATH="${ARTIFACT_DIR}/${ARTIFACT_STEM}.tar.gz"
echo "[release.sh] Packaging artifact at ${ARCHIVE_PATH}"
tar -C "${TMP_DIR}" -czf "${ARCHIVE_PATH}" "${ARTIFACT_STEM}"

NOTES_ARGS=()
if [[ -n "${RELEASE_NOTES_FILE:-}" ]]; then
  NOTES_ARGS=(--notes-file "${RELEASE_NOTES_FILE}")
elif [[ -n "${RELEASE_NOTES:-}" ]]; then
  NOTES_ARGS=(--notes "${RELEASE_NOTES}")
else
  NOTES_ARGS=(--notes "Automated release for ${TAG}")
fi

echo "[release.sh] Publishing ${ARCHIVE_PATH} to ${REPO} (tag: ${TAG})"
if gh release view "${TAG}" -R "${REPO}" >/dev/null 2>&1; then
  gh release upload "${TAG}" "${ARCHIVE_PATH}" --clobber -R "${REPO}"
else
  gh release create "${TAG}" "${ARCHIVE_PATH}" -R "${REPO}" -t "${RELEASE_NAME}" "${NOTES_ARGS[@]}"
fi

echo "[release.sh] Release published successfully."
