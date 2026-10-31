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

# The same trick does not help a field that is `required` AND nullable (`#[schema(required = true)] Option<T>`): the generated
# type would insist on a value, and `null` would fail to decode (an environment draft has no `draft` while the agent makes
# the first one). Such a property is dropped from its schema's `required` list, so it is generated optional and `null` decodes.
python3 - "$spec" "$work/openapi.yaml" <<'PY'
import re, sys
src, out = sys.argv[1], sys.argv[2]
lines = open(src).read().split("\n")
nullable = {}  # (schema, property) pairs written as oneOf [null, ref]
schema = prop = None
for n, line in enumerate(lines):
    m = re.match(r"^    (\w+):$", line)
    if m: schema, prop = m.group(1), None
    m = re.match(r"^        (\w+):$", line)
    if m: prop = m.group(1)
    if re.match(r"^          - type: 'null'$", line) and prop and lines[n-1].strip() == "oneOf:":
        nullable.setdefault(schema, set()).add(prop)
res, schema, in_required = [], None, False
for line in lines:
    m = re.match(r"^    (\w+):$", line)
    if m: schema, in_required = m.group(1), False
    if re.match(r"^      required:$", line): in_required = True
    elif in_required:
        m = re.match(r"^      - (\w+)$", line)
        if m:
            if m.group(1) in nullable.get(schema, ()): continue
        else: in_required = False
    res.append(line)
open(out, "w").write("\n".join(res))
PY
# the oneOf/null pair itself is replaced by the plain `$ref` below
perl -0pi -e 's/^( *)oneOf:\n\1- type: .null.\n\1- (\$ref: [^\n]+)\n(?:\1  [a-z][^\n]*\n)*/$1$2\n/mg' "$work/openapi.yaml"
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
