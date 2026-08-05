#!/usr/bin/env bash
# Runs the desktop app against a throwaway state directory, so a development
# session never touches the real login, keys, or usage log.
set -euo pipefail
export AUTH2API_HOME="${AUTH2API_HOME:-/tmp/auth2api-dev}"
mkdir -p "$AUTH2API_HOME"
echo "state: $AUTH2API_HOME"
exec cargo run -p auth2api-desktop "$@"
