#!/bin/sh
# Regenerates synthetic.jsonl from a throwaway beads repo.
# Usage: make-synthetic.sh <output path>
set -eu

out=$(cd "$(dirname "$1")" && pwd)/$(basename "$1")
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
cd "$tmp"

git init -q
bd init --prefix fx --skip-agents --skip-hooks --non-interactive -q </dev/null
bd config set status.custom review >/dev/null
bd config set types.custom incident >/dev/null

e=$(bd create --silent --type epic --priority 1 \
  --title "Epic with #tag and %% comment" --description "Body with **markdown**.")
a=$(bd create --silent --parent "$e" --type task --priority 0 \
  --title 'Child: quotes " and [[brackets]]' \
  --design "design text" --acceptance "acc" --notes "notes" --labels "alpha,beta")
b=$(bd create --silent --parent "$e" --type bug --priority 4 \
  --title "Second child" --deps "blocks:$a")
m=$(bd create --silent --type molecule --title "A molecule")
c=$(bd create --silent --type incident --priority 2 --title "Custom type")

bd dep add "$c" "$m" --type parent-child >/dev/null
bd dep add "$c" "$a" --type relates-to >/dev/null
bd dep add "$c" "$b" --type discovered-from >/dev/null
bd dep add "$c" "$e" --type supersedes >/dev/null
bd dep add "$b" "$m" --type waits-for >/dev/null

bd comments add "$a" "First comment
with two lines" >/dev/null
bd comments add "$a" "Second" >/dev/null

bd update "$a" --status in_progress --assignee someone >/dev/null
bd update "$b" --status review >/dev/null
bd update "$c" --status deferred >/dev/null
bd close "$e" --force --reason "done for now" >/dev/null 2>&1
bd create --silent --ephemeral --title "A wisp" >/dev/null

bd export --all | sed 's/[A-Za-z0-9._%+-]*@[A-Za-z0-9.-]*\.[A-Za-z]*/owner@example.com/g' > "$out"
