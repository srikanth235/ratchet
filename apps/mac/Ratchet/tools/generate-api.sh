#!/bin/bash
# Regenerates Sources/RatchetAPI (types and client) from ../../../api/openapi.yaml with swift-openapi-generator.
# The output is committed: the build never runs the generator (no build plugin, no swift-syntax compile in CI).
# Run it whenever api/openapi.yaml changes (`cargo run -p ratchetd -- openapi` writes that file), then commit the result.
#
# One preparation step: utoipa writes an optional field as `oneOf: [{type: 'null'}, {$ref: X}]`, which the generator
# skips with a warning (the field would be missing from the Swift type). The field is never `required`, and a generated
# optional decodes JSON null as nil, so the script replaces that pair by the plain `$ref`.
set -euo pipefail
here=$(cd "$(dirname "$0")/.." && pwd)
spec="$here/../../../api/openapi.yaml"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

perl -0pe 's/^( *)oneOf:\n\1- type: .null.\n\1- (\$ref: [^\n]+)\n(?:\1  [a-z][^\n]*\n)*/$1$2\n/mg' "$spec" > "$work/openapi.yaml"
if grep -q "^ *- type: 'null'" "$work/openapi.yaml"; then
  echo "warning: a nullable oneOf is left in the spec; the generator will skip that field"
fi

out="$here/Sources/RatchetAPI"
mkdir -p "$out"
rm -f "$out"/Types*.swift "$out"/Client.swift
swift run --package-path "$here/tools/api" --scratch-path "$HOME/Library/Developer/Ratchet/api-generator" \
  swift-openapi-generator generate "$work/openapi.yaml" \
  --config "$here/tools/api/openapi-generator-config.yaml" --output-directory "$out"
echo "generated $(ls "$out" | wc -l | tr -d ' ') files in Sources/RatchetAPI"
