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

TAG=$(git describe --match=NeVeRmAtCh --always --abbrev=0 --dirty)
ORG="jitolabs"

TAG=$TAG ORG=$ORG docker compose --env-file="${ENV_FILE}" down

echo -e "\e[1;32mProxy server has stopped.\e[0m"
