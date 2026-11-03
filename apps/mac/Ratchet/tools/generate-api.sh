#!/bin/bash
# Regenerates Sources/RatchetAPI (types and client) from ../../../api/openapi.yaml with swift-openapi-generator.
# The output is committed: the build never runs the generator (no build plugin, no swift-syntax compile in CI).
# Run it whenever api/openapi.yaml changes (`cargo run -p ratchetd -- openapi` writes that file), then commit the result.
#
# One preparation step (tools/openapi-for-swift.py): utoipa writes an optional field as `oneOf: [{type: 'null'}, {$ref: X}]`,
# which the generator skips with a warning (the field would be missing from the Swift type). The script replaces that pair by the
# plain `$ref` and takes the field out of `required`, so the generated property is an optional that decodes both a missing key
# and JSON null (a non-optional one would fail to decode a `null` the daemon always sends).
set -euo pipefail
here=$(cd "$(dirname "$0")/.." && pwd)
spec="$here/../../../api/openapi.yaml"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

python3 "$here/tools/openapi-for-swift.py" < "$spec" > "$work/openapi.yaml"
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
