#!/usr/bin/env python3
"""Prepares api/openapi.yaml for swift-openapi-generator (see generate-api.sh). Reads the document on stdin, writes it on stdout.

utoipa writes a nullable reference as

    name:
      oneOf:
      - type: 'null'
      - $ref: '#/components/schemas/X'
      description: ...

which the generator skips with a warning (the field would be missing from the Swift type). The preparation replaces the pair by
the plain `$ref` (and drops the sibling keys, `description:` included, also when it is a multi-line block scalar). A plain
reference that is listed in `required` becomes a non-optional Swift property that cannot decode JSON `null`, so the name is also
removed from the `required` list of the schema the property belongs to: a generated optional decodes both a missing key and `null`.
Only the standard library; the document is processed line by line (utoipa's YAML is regular: two-space indent, `required` before
`properties`).
"""
import re
import sys

lines = sys.stdin.read().split("\n")
out = []
nullable_ref = re.compile(r"^( *)oneOf:$")
removed = []  # (properties_line_index_in_out, name)


def indent(s):
    return len(s) - len(s.lstrip(" "))


i = 0
while i < len(lines):
    line = lines[i]
    m = nullable_ref.match(line)
    if (
        m
        and i + 2 < len(lines)
        and lines[i + 1] == f"{m.group(1)}- type: 'null'"
        and lines[i + 2].startswith(f"{m.group(1)}- $ref: ")
        and out
    ):
        ind = len(m.group(1))
        # the property name line is the one just written, one level up
        prop = out[-1]
        name = prop.strip().rstrip(":")
        out.append(f"{' ' * ind}{lines[i + 2][ind + 2:]}")
        removed.append((len(out) - 1, name, ind - 2))
        i += 3
        # drop the sibling keys of the property (indent == ind) and their continuation lines (indent > ind)
        while i < len(lines) and lines[i].strip() != "" and indent(lines[i]) >= ind and not lines[i].lstrip().startswith("- "):
            i += 1
        continue
    out.append(line)
    i += 1

# remove each name from the `required` list of its schema (the `properties:` key one level up from the property)
drop = set()
touched = {}  # index of a `required:` line -> indexes of its list items
for idx, name, prop_indent in removed:
    pad = " " * (prop_indent - 2)
    j = idx
    while j >= 0 and not (out[j] == f"{pad}properties:"):
        j -= 1
    if j < 0:
        continue
    k = j - 1
    while k >= 0 and indent(out[k]) >= len(pad) and out[k].strip() != "":
        if out[k] == f"{pad}required:":
            items = []
            n = k + 1
            while n < len(out) and out[n].startswith(f"{pad}- "):
                items.append(n)
                if out[n] == f"{pad}- {name}":
                    drop.add(n)
                n += 1
            touched[k] = items
            break
        k -= 1

# a `required:` that lost every entry would be an empty list (a property can itself be called `required`: only touch the keys above)
for k, items in touched.items():
    if items and all(n in drop for n in items):
        drop.add(k)

sys.stdout.write("\n".join(l for n, l in enumerate(out) if n not in drop))
