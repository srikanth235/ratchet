/**
 * A small ICU-style message compiler: `{name}`, `{n, plural, one {# task} other {# tasks}}` and
 * `{kind, select, a {...} other {...}}`. A message compiles to a function of its variables.
 */
export type Vars = Record<string, string | number>;
export type Message = (vars?: Vars) => string;

type Node =
  | string
  | { type: 'arg'; name: string }
  | { type: 'hash' }
  | { type: 'plural' | 'select'; name: string; cases: Map<string, Node[]> };

class Parser {
  private pos = 0;
  constructor(private readonly src: string) {}

  parse(): Node[] {
    const nodes = this.sequence(false);
    if (this.pos < this.src.length) throw new SyntaxError(`unexpected "}" at ${this.pos} in ${JSON.stringify(this.src)}`);
    return nodes;
  }

  private sequence(inCase: boolean): Node[] {
    const nodes: Node[] = [];
    let text = '';
    const flush = () => {
      if (text) nodes.push(text);
      text = '';
    };
    while (this.pos < this.src.length) {
      const ch = this.src[this.pos]!;
      if (ch === '}') {
        if (inCase) break;
        throw new SyntaxError(`unbalanced "}" at ${this.pos} in ${JSON.stringify(this.src)}`);
      }
      if (ch === '{') {
        flush();
        nodes.push(this.argument());
      } else if (ch === '#' && inCase) {
        flush();
        nodes.push({ type: 'hash' });
        this.pos++;
      } else {
        text += ch;
        this.pos++;
      }
    }
    flush();
    return nodes;
  }

  private argument(): Node {
    this.pos++; // {
    const name = this.word();
    this.skipSpace();
    if (this.src[this.pos] === '}') {
      this.pos++;
      return { type: 'arg', name };
    }
    if (this.src[this.pos] !== ',') throw new SyntaxError(`expected "," or "}" at ${this.pos} in ${JSON.stringify(this.src)}`);
    this.pos++;
    this.skipSpace();
    const kind = this.word();
    if (kind !== 'plural' && kind !== 'select') throw new SyntaxError(`unknown argument type ${JSON.stringify(kind)}`);
    this.skipSpace();
    if (this.src[this.pos] !== ',') throw new SyntaxError(`expected "," after ${kind} at ${this.pos}`);
    this.pos++;
    const cases = new Map<string, Node[]>();
    for (;;) {
      this.skipSpace();
      if (this.src[this.pos] === '}') {
        this.pos++;
        break;
      }
      const label = this.word();
      this.skipSpace();
      if (this.src[this.pos] !== '{') throw new SyntaxError(`expected "{" after case ${JSON.stringify(label)}`);
      this.pos++;
      cases.set(label, this.sequence(true));
      if (this.src[this.pos] !== '}') throw new SyntaxError(`unterminated case ${JSON.stringify(label)}`);
      this.pos++;
    }
    if (!cases.has('other')) throw new SyntaxError(`${kind} argument ${JSON.stringify(name)} needs an "other" case`);
    return { type: kind, name, cases };
  }

  private word(): string {
    const start = this.pos;
    while (this.pos < this.src.length && /[\w=.-]/.test(this.src[this.pos]!)) this.pos++;
    if (start === this.pos) throw new SyntaxError(`expected a name at ${this.pos} in ${JSON.stringify(this.src)}`);
    return this.src.slice(start, this.pos);
  }

  private skipSpace(): void {
    while (this.pos < this.src.length && this.src[this.pos] === ' ') this.pos++;
  }
}

function render(nodes: Node[], vars: Vars, hash: string): string {
  let out = '';
  for (const n of nodes) {
    if (typeof n === 'string') out += n;
    else if (n.type === 'hash') out += hash;
    else if (n.type === 'arg') out += String(vars[n.name] ?? `{${n.name}}`);
    else {
      const v = vars[n.name];
      const key = n.type === 'plural' ? (v === 1 ? 'one' : 'other') : String(v);
      const body = n.cases.get(`=${String(v)}`) ?? n.cases.get(key) ?? n.cases.get('other')!;
      out += render(body, vars, String(v));
    }
  }
  return out;
}

export function compileMessage(source: string): Message {
  const nodes = new Parser(source).parse();
  if (nodes.length === 0) return () => '';
  if (nodes.length === 1 && typeof nodes[0] === 'string') {
    const text = nodes[0];
    return () => text;
  }
  return (vars = {}) => render(nodes, vars, '');
}
