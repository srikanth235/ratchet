import { describe, expect, it } from 'vitest';
import { literalNumber, literalString, parseHcl, stripComments, walkBlocks } from './hcl.js';

describe('parseHcl', () => {
  it('reads blocks, labels, attributes and nested blocks', () => {
    const f = parseHcl(`
      resource "aws_s3_object" "seed" {
        bucket = module.sandbox.artifacts_bucket
        key    = "env/seed.sql"
        count  = 2
        flag   = true
        nothing = null
        lifecycle {
          prevent_destroy = false
        }
      }
      variable "x" {}
    `);
    expect(f.errors).toEqual([]);
    expect(f.blocks.map((b) => [b.type, ...b.labels])).toEqual([['resource', 'aws_s3_object', 'seed'], ['variable', 'x']]);
    const r = f.blocks[0]!;
    expect(r.attrs['bucket']).toEqual({ kind: 'expr', text: 'module.sandbox.artifacts_bucket' });
    expect(literalString(r.attrs['key'])).toBe('env/seed.sql');
    expect(literalNumber(r.attrs['count'])).toBe(2);
    expect(r.attrs['flag']).toEqual({ kind: 'bool', value: true });
    expect(r.attrs['nothing']).toEqual({ kind: 'null' });
    expect(r.blocks[0]!.type).toBe('lifecycle');
    expect(r.blocks[0]!.attrs['prevent_destroy']).toEqual({ kind: 'bool', value: false });
  });

  it('reads nested objects with newline or comma separators, function calls and heredocs', () => {
    const f = parseHcl(`
module "sandbox" {
  source = "./ratchet-sandbox"
  machines = {
    api = { role = "app", instance_type = "c7i.large" }
    db = {
      role          = "database"
      user_data     = templatefile("\${path.module}/db.sh", { bucket = var.ratchet.artifacts_bucket })
      detail        = <<-EOT
        hello # not a comment
        } stray brace in a heredoc
      EOT
    }
  }
}
`);
    expect(f.errors).toEqual([]);
    const m = f.blocks[0]!.attrs['machines']!;
    expect(m.kind).toBe('object');
    if (m.kind !== 'object') return;
    expect(Object.keys(m.entries)).toEqual(['api', 'db']);
    const api = m.entries['api']!;
    expect(api.kind === 'object' && literalString(api.entries['instance_type'])).toBe('c7i.large');
    const db = m.entries['db']!;
    expect(db.kind).toBe('object');
    if (db.kind !== 'object') return;
    expect(db.entries['user_data']!.kind).toBe('expr');
    expect((db.entries['detail'] as { value: string }).value).toContain('stray brace in a heredoc');
  });

  it('ignores comments of every style but keeps strings that look like them', () => {
    const f = parseHcl(`
# a comment with { a brace
// another one }
/* multi
   line { */
locals {
  url = "http://example.test/#not-a-comment"
  note = "a // b"
}
`);
    expect(f.errors).toEqual([]);
    expect(literalString(f.blocks[0]!.attrs['url'])).toBe('http://example.test/#not-a-comment');
    expect(literalString(f.blocks[0]!.attrs['note'])).toBe('a // b');
    expect(stripComments('a # b\nc')).toBe('a    \nc');
  });

  it('flags interpolated strings as dynamic', () => {
    const f = parseHcl('locals {\n  a = "x-${var.y}"\n  b = "plain"\n}\n');
    expect(f.blocks[0]!.attrs['a']).toMatchObject({ kind: 'string', dynamic: true });
    expect(literalString(f.blocks[0]!.attrs['a'])).toBeUndefined();
    expect(literalString(f.blocks[0]!.attrs['b'])).toBe('plain');
  });

  it('reports an unclosed block instead of throwing', () => {
    const f = parseHcl('resource "a" "b" {\n  x = 1\n');
    expect(f.errors.length).toBeGreaterThan(0);
    expect(f.errors[0]).toMatch(/never closed/);
  });

  it('records the source line of each block and walks nested blocks', () => {
    const f = parseHcl('\n\nresource "a" "b" {\n  provisioner "local-exec" {\n    command = "x"\n  }\n}\n');
    expect(f.blocks[0]!.line).toBe(3);
    expect([...walkBlocks(f.blocks)].map((b) => b.type)).toEqual(['resource', 'provisioner']);
  });
});
