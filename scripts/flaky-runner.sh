#!/usr/bin/env bash
set -euo pipefail

if [[ $# -eq 0 ]]; then
  echo "Usage: $0 <command> [args...]" >&2
  exit 1
fi

child_pid=""

cleanup() {
  if [[ -n "${child_pid}" ]] && kill -0 "${child_pid}" 2>/dev/null; then
    kill "${child_pid}" 2>/dev/null || true
    wait "${child_pid}" 2>/dev/null || true
  fi
}

trap cleanup EXIT INT TERM

run_command() {
  "$@" &
  child_pid=$!
}

while true; do
  run_command "$@"

  while kill -0 "${child_pid}" 2>/dev/null; do
    sleep $((10 +$RANDOM%10))
    if (( RANDOM % 100 < 5 )); then
      echo -e "[flaky] \033[0;31mKilling process ${child_pid}\033[0m" >&2
      kill "${child_pid}" 2>/dev/null || true
      wait "${child_pid}" 2>/dev/null || true
      child_pid=""
      delay=$((10 +$RANDOM%20))

      echo -e "[flaky] \033[0;34mSleeping ${delay}s before restart\033[0m " >&2
      sleep $delay

      continue 2
    fi
  done

  set +e
  wait "${child_pid}"
  exit_code=$?
  set -e

  child_pid=""
  exit "${exit_code}"
done
