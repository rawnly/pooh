#!/usr/bin/env bash
# Generate shell completions and the man page into dist/.
#   scripts/generate.sh [path-to-pooh]
# Completions come from the binary itself (usage-rs); the man page is rendered
# from the binary's usage spec by the `usage` CLI (https://usage.jdx.dev).
set -euo pipefail

cd "$(dirname "$0")/.."
bin="${1:-}"
if [ -z "$bin" ]; then
  cargo build --release --quiet
  bin=target/release/pooh
fi

out=dist
mkdir -p "$out/completions" "$out/man"

"$bin" completions bash       > "$out/completions/pooh.bash"
"$bin" completions zsh        > "$out/completions/_pooh"
"$bin" completions fish       > "$out/completions/pooh.fish"
"$bin" completions elvish     > "$out/completions/pooh.elv"
"$bin" completions nu         > "$out/completions/pooh.nu"
"$bin" completions powershell > "$out/completions/pooh.ps1"
"$bin" spec       > "$out/pooh.usage.kdl"

if command -v usage >/dev/null; then
  usage generate manpage -f "$out/pooh.usage.kdl" -o "$out/man/pooh.1"
else
  echo "warning: \`usage\` CLI not found, skipping man page (mise use -g usage)" >&2
fi

echo "generated in $out/:"; find "$out" -type f | sort
