#!/usr/bin/env bash
set -euo pipefail

ALBUM="${HOME}/album"
VAULT="${HOME}/vault"
NOTES="${HOME}/notes.txt"
FEH_PID=""

cleanup() {
    set +e
    if [[ -n "$FEH_PID" ]]; then
        kill "$FEH_PID" 2>/dev/null || true
        wait "$FEH_PID" 2>/dev/null || true
    fi
    if mountpoint -q "$VAULT"; then
        albumfs umount "$VAULT" >/dev/null 2>&1 || true
    fi
}
trap cleanup EXIT

type_line() {
    printf '$ '
    local text="$1"
    local i
    for ((i = 0; i < ${#text}; i++)); do
        printf '%s' "${text:i:1}"
        sleep 0.012
    done
    printf '\n'
}

run() {
    type_line "$1"
    eval "$1"
    printf '\n'
    sleep 0.55
}

wait_for_mount() {
    local i
    for ((i = 0; i < 50; i++)); do
        if mountpoint -q "$VAULT"; then
            return 0
        fi
        sleep 0.1
    done
    printf 'mount did not become ready\n' >&2
    return 1
}

wait_for_unmount() {
    local i
    for ((i = 0; i < 50; i++)); do
        if ! mountpoint -q "$VAULT"; then
            return 0
        fi
        sleep 0.1
    done
    printf 'mount did not close\n' >&2
    return 1
}

clear
run "ls ~/album"
run "albumfs format --anchor cover.jpg --passphrase demo ~/album"
run "albumfs stats --anchor cover.jpg --passphrase demo ~/album"
run "albumfs mount --anchor cover.jpg --passphrase demo ~/album ~/vault &"
wait_for_mount
run "ls -A ~/vault"
run "cp ~/notes.txt ~/vault/ && mkdir ~/vault/private"
run "ls ~/vault"
run "cat ~/vault/notes.txt"
run "albumfs umount ~/vault"
wait_for_unmount
run "ls -A ~/vault"

type_line "feh --fullscreen ~/album/cover.jpg"
feh --fullscreen "$ALBUM/cover.jpg" &
FEH_PID=$!
sleep 3
kill "$FEH_PID" 2>/dev/null || true
wait "$FEH_PID" 2>/dev/null || true
FEH_PID=""

run "albumfs mount --anchor cover.jpg --passphrase demo ~/album ~/vault &"
wait_for_mount
run "ls ~/vault"
run "cat ~/vault/notes.txt"
run "albumfs umount ~/vault"
wait_for_unmount
sleep 1
