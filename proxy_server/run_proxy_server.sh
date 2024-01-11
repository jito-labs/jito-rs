#!/usr/bin/env bash
set -euxo pipefail

ENV_FILE=./config/.env
if [[ -f $ENV_FILE ]]; then
  # A little hacky, but .env files can't execute so this is the best we have for now
  # shellcheck disable=SC2046
  export $(grep -v '#' "$ENV_FILE" | awk '/=/ {print $1}')
else
  echo ".env file not found at ${ENV_FILE}"
  exit 1
fi

# Some container vars
TAG=$(git describe --match=NeVeRmAtCh --always --abbrev=0 --dirty)
ORG="jitolabs"

export DOCKER_LOCALHOST=172.17.0.1
if [[ $(uname) == "Darwin" ]]; then
  DOCKER_LOCALHOST=docker.for.mac.localhost
fi

# Build and run
DOCKER_BUILDKIT=1 \
  BUILDKIT_PROGRESS=plain \
  TAG=$TAG \
  ORG=$ORG \
  docker compose --env-file="${ENV_FILE}" up --build --remove-orphans --renew-anon-volumes  -d

# Set a timeout (in seconds)
timeout=300
start_time=$(date +%s)

# Wait for the container to be up with timeout
while [ "$(TAG=$TAG ORG=$ORG docker compose --env-file="${ENV_FILE}" ps -q | xargs docker inspect --format '{{.State.Status}}' | grep -c "running")" -eq 0 ]; do
  # Check if the timeout has been reached
  current_time=$(date +%s)
  elapsed_time=$((current_time - start_time))
  if [ $elapsed_time -ge $timeout ]; then
    echo "Timeout reached. Container did not start within $timeout seconds."
    exit 1
  fi

  # Sleep for a while before checking again
  sleep 5
done

# Tag the image as latest
IMAGE_NAME="$(TAG=$TAG ORG=$ORG docker compose --env-file="${ENV_FILE}" ps -q | xargs docker inspect --format '{{.Config.Image}}')"
docker image tag "$IMAGE_NAME" "${IMAGE_NAME%%:*}":latest

echo -e "\e[1;32mProxy server is up and running.\e[0m"
