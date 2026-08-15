#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."

if ! command -v docker >/dev/null 2>&1; then
  echo "docker is required to build production images" >&2
  exit 127
fi
for artifact in \
  target/release/wareboxes-server \
  target/release/wareboxes-worker \
  target/site/pkg/wareboxes-web.css \
  target/site/pkg/wareboxes-web.js \
  target/site/pkg/wareboxes-web.wasm; do
  if [ ! -f "$artifact" ]; then
    echo "release artifact is missing: $artifact; run scripts/build-web.sh first" >&2
    exit 1
  fi
done

image_prefix="${WAREBOXES_IMAGE_PREFIX:-wareboxes}"
image_tag="${WAREBOXES_IMAGE_TAG:-local}"
image_source="${WAREBOXES_IMAGE_SOURCE:-}"
image_revision="${WAREBOXES_IMAGE_REVISION:-$(git rev-parse HEAD)}"
if [[ ! "$image_prefix" =~ ^[a-z0-9][a-z0-9._/-]{0,127}$ ]]; then
  echo "WAREBOXES_IMAGE_PREFIX is not a valid lowercase image prefix" >&2
  exit 2
fi
if [[ ! "$image_tag" =~ ^[A-Za-z0-9_][A-Za-z0-9_.-]{0,127}$ ]]; then
  echo "WAREBOXES_IMAGE_TAG is not a valid image tag" >&2
  exit 2
fi
if [[ ! "$image_revision" =~ ^[0-9a-f]{40}$ ]]; then
  echo "WAREBOXES_IMAGE_REVISION must be a 40-character Git SHA" >&2
  exit 2
fi

image_context="$(mktemp -d)"
cleanup() {
  rm -r "$image_context"
}
trap cleanup EXIT

install -m 0644 deploy/Dockerfile "$image_context/Dockerfile"
install -m 0755 target/release/wareboxes-server "$image_context/wareboxes-server"
install -m 0755 target/release/wareboxes-worker "$image_context/wareboxes-worker"
cp -a target/site "$image_context/site"

common_args=(
  --file "$image_context/Dockerfile"
  --build-arg "OCI_SOURCE=$image_source"
  --build-arg "OCI_REVISION=$image_revision"
)
docker build \
  "${common_args[@]}" \
  --target api \
  --tag "$image_prefix-api:$image_tag" \
  "$image_context"
docker build \
  "${common_args[@]}" \
  --target worker \
  --tag "$image_prefix-worker:$image_tag" \
  "$image_context"

api_user="$(docker image inspect --format '{{.Config.User}}' "$image_prefix-api:$image_tag")"
worker_user="$(docker image inspect --format '{{.Config.User}}' "$image_prefix-worker:$image_tag")"
healthcheck="$(
  docker image inspect --format '{{json .Config.Healthcheck.Test}}' "$image_prefix-api:$image_tag"
)"
if [ "$api_user" != "10001:10001" ] || [ "$worker_user" != "10001:10001" ]; then
  echo "production images must run as the dedicated non-root identity" >&2
  exit 1
fi
if [[ "$healthcheck" != *health/ready* ]]; then
  echo "API image healthcheck must use database-and-schema readiness" >&2
  exit 1
fi
echo "built $image_prefix-api:$image_tag and $image_prefix-worker:$image_tag"
