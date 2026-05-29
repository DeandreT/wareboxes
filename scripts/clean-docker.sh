#!/usr/bin/env bash
# Stop and remove Wareboxes Docker resources.
set -euo pipefail
cd "$(dirname "$0")/.."

usage() {
  cat <<'USAGE'
Usage:
  scripts/clean-docker.sh             stop/remove this repo's Compose containers and networks
  scripts/clean-docker.sh --volumes   also remove this repo's Compose volumes/data
  scripts/clean-docker.sh --global    stop/remove every Docker container and unused network on this machine

The default is intentionally project-scoped so unrelated Docker workloads are not destroyed.
USAGE
}

include_volumes=false
global=false

for arg in "$@"; do
  case "$arg" in
    --volumes|-v) include_volumes=true ;;
    --global) global=true ;;
    --help|-h)
      usage
      exit 0
      ;;
    *) echo "unknown argument: $arg" >&2; exit 2 ;;
  esac
done

if ! command -v docker >/dev/null 2>&1; then
  echo "docker not found. Install Docker before cleaning Docker resources." >&2
  exit 127
fi

if ! docker info >/dev/null 2>&1; then
  echo "docker is not available to this user. Start Docker and make sure your user can access /var/run/docker.sock." >&2
  echo "On Linux: sudo usermod -aG docker \"$USER\", then log out and back in." >&2
  exit 1
fi

if [ "$global" = true ]; then
  echo "Stopping all Docker containers..."
  container_ids="$(docker ps -aq)"
  if [ -n "$container_ids" ]; then
    # shellcheck disable=SC2086
    docker stop $container_ids >/dev/null
    # shellcheck disable=SC2086
    docker rm $container_ids >/dev/null
  fi

  echo "Removing unused Docker networks..."
  docker network prune -f >/dev/null

  if [ "$include_volumes" = true ]; then
    echo "Removing unused Docker volumes..."
    docker volume prune -f >/dev/null
  fi

  echo "Docker global cleanup complete."
  exit 0
fi

compose_args=(down --remove-orphans)
if [ "$include_volumes" = true ]; then
  compose_args+=(-v)
fi

echo "Stopping Wareboxes Docker Compose resources..."
docker compose "${compose_args[@]}"
echo "Wareboxes Docker cleanup complete."
