#!/usr/bin/env bash
# Release smoke test: the everyday flow with the installed binary, the way an agent drives it
# (no TTY, CI set). Needs `poenv` on PATH and a fresh POENV_HOME.
#   bash scripts/smoke-flow.sh
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
fail() { echo "FAIL: $*" >&2; exit 1; }
# expect CODE CMD…: runs CMD, fails unless it exits with CODE.
expect() {
  local want=$1 code=0
  shift
  "$@" > out.txt 2>&1 || code=$?
  [ "$code" = "$want" ] || { cat out.txt; fail "\"$*\" exited $code, expected $want"; }
}
has() { grep -qF -- "$2" "$1" || { cat "$1"; fail "$1 should contain: $2"; }; }
lacks() { ! grep -qF -- "$2" "$1" || { cat "$1"; fail "$1 should not contain: $2"; }; }

[ -n "${POENV_HOME:-}" ] || fail "set POENV_HOME to an empty folder"
export CI=true

echo "== vault"
repo=$(mktemp -d)/smoke-app
mkdir -p "$repo" && cd "$repo" # before any expect: it writes out.txt here
git init -q
echo '{"dependencies":{"next":"15.0.0"}}' > package.json
printf 'DB=postgres://localhost/dev\nAPI_KEY=sk-local-123\n' > .env.local
expect 5 poenv init --json # no vault yet: only a person can create it
cargo run -q --manifest-path "$here/../Cargo.toml" --example smoke_vault

echo "== init imports the env file"
expect 0 poenv init --json --yes
has out.txt '"imported":["API_KEY","DB"]'
has .env.local "# app: smoke-app  env: local"
has .gitignore "/.env.local"
[ -f .poenv ] || fail ".poenv not written"

echo "== set and use"
printf '%s' "postgres://staging-host/app" | poenv set DB --env staging > out.txt 2>&1 || { cat out.txt; fail "set"; }
expect 1 poenv set DB never-as-argument
expect 0 poenv use staging --json
has .env.local "env: staging"
has .env.local "postgres://staging-host/app"
lacks out.txt "postgres://staging-host/app"

echo "== import pasted keys"
printf 'DB="postgres://pasted/db"\nNEW_KEY="a b"\nEMPTY=""\n' >> .env.local
expect 0 poenv import --json
has out.txt '"imported":["DB","EMPTY","NEW_KEY"]'
has out.txt '"overwritten":["DB"]'
[ "$(grep -c '^DB=' .env.local)" = 1 ] || { cat .env.local; fail "DB should be in the file once"; }
has .env.local "postgres://pasted/db"
expect 1 poenv import

echo "== shared keys"
printf 'v1' | poenv set TOKEN --shared > out.txt 2>&1 || { cat out.txt; fail "set --shared"; }
expect 0 poenv add TOKEN
has .env.local "TOKEN=v1"
printf 'v2' | poenv set TOKEN --shared > out.txt 2>&1 || { cat out.txt; fail "rotate shared"; }
has .env.local "TOKEN=v2"

echo "== run injects without touching the file"
expect 0 poenv run local -- node -e "process.exit(process.env.DB === 'postgres://localhost/dev' ? 0 : 1)"
has .env.local "env: staging"
# Run from a script, values in the command's output are masked.
expect 0 poenv run local -- node -e "console.log(process.env.DB)"
has out.txt "***"
lacks out.txt "postgres://"

echo "== env management"
expect 0 poenv env new qa --from staging
expect 0 poenv env rename qa test
expect 0 poenv ls --json
has out.txt '"test"'
lacks out.txt "postgres://"

echo "== agents are refused what belongs to the person"
expect 4 poenv use prod
expect 4 poenv get DB
expect 4 poenv backup
expect 4 poenv restore backup.enc
lacks out.txt "postgres://"

echo "== values run keeps the env file to key names"
expect 0 poenv values run
lacks .env.local "postgres://"
expect 0 poenv run local -- node -e "process.exit(process.env.DB === 'postgres://localhost/dev' ? 0 : 1)"
expect 4 poenv values file
lacks .env.local "postgres://"

echo "== doctor"
expect 0 poenv status --json
has out.txt '"env":"staging"'
code=0; poenv doctor --json > out.txt 2>&1 || code=$?
[ "$code" = 0 ] || [ "$code" = 5 ] || { cat out.txt; fail "doctor exited $code"; }

echo "smoke flow passed"
