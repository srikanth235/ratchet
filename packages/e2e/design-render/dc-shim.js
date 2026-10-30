/*
 * Minimal browser runtime for the Ratchet `.dc.html` design components. It stands in for the proprietary
 * `./support.js` runtime and supports exactly what the design files use:
 *
 *   - `<script type="text/x-dc">class Component extends DCLogic { renderVals() { … } }</script>`
 *   - `{{dotted.path}}` holes in text and in attribute values
 *   - `<sc-for list="{{x}}" as="item">…</sc-for>` (nestable) and `<sc-if value="{{c}}">…</sc-if>`
 *   - `<helmet>` (hoisted into <head>)
 *
 * The template is expanded as a *string* before it is parsed into DOM. Going through the HTML parser first would
 * foster-parent `<sc-for>` out of `<tbody>`/`<table>` and drop the `<tr>`s inside it.
 *
 * The harness embeds the original file as `window.__DC_SOURCE__`; this script replaces the document body with the
 * rendered result on DOMContentLoaded.
 */
(function () {
  'use strict';

  /** Base class of design components: props/state only, no lifecycle. */
  class DCLogic {
    constructor(props) {
      this.props = props || {};
      this.state = {};
    }
    setState(patch) {
      this.state = Object.assign({}, this.state, patch);
    }
    renderVals() {
      return {};
    }
  }

  const esc = (v) =>
    String(v).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');

  function lookup(scope, path) {
    let cur = scope;
    for (const key of path.split('.')) {
      if (cur === null || cur === undefined) return undefined;
      cur = cur[key];
    }
    return cur;
  }

  const HOLE = /\{\{\s*([A-Za-z_$][\w$]*(?:\.[\w$]+)*)\s*\}\}/g;
  const interpolate = (text, scope) =>
    text.replace(HOLE, (_, path) => {
      const v = lookup(scope, path);
      return v === undefined || v === null ? '' : esc(v);
    });

  /** Index of the matching `</tag>` for the `<tag …>` whose body starts at `from` (handles nesting). */
  function findClose(src, tag, from) {
    const open = new RegExp(`<${tag}(\\s|>)`, 'g');
    const close = `</${tag}>`;
    let depth = 1;
    let i = from;
    while (depth > 0) {
      const nextClose = src.indexOf(close, i);
      if (nextClose < 0) throw new Error(`unclosed <${tag}>`);
      open.lastIndex = i;
      const m = open.exec(src);
      if (m && m.index < nextClose) {
        depth++;
        i = m.index + m[0].length;
      } else {
        depth--;
        i = nextClose + close.length;
        if (depth === 0) return nextClose;
      }
    }
    return -1;
  }

  const attr = (tagText, name) => {
    const m = new RegExp(`${name}="([^"]*)"`).exec(tagText);
    return m ? m[1] : null;
  };
  const holePath = (s) => {
    const m = /^\s*\{\{\s*([^}\s]+)\s*\}\}\s*$/.exec(s || '');
    return m ? m[1] : s;
  };

  function expand(src, scope) {
    let out = '';
    let i = 0;
    const tagRe = /<(sc-for|sc-if)\b([^>]*)>/g;
    for (;;) {
      tagRe.lastIndex = i;
      const m = tagRe.exec(src);
      if (!m) break;
      out += interpolate(src.slice(i, m.index), scope);
      const bodyStart = m.index + m[0].length;
      const bodyEnd = findClose(src, m[1], bodyStart);
      const body = src.slice(bodyStart, bodyEnd);
      if (m[1] === 'sc-for') {
        const list = lookup(scope, holePath(attr(m[2], 'list')));
        const as = attr(m[2], 'as') || 'item';
        (Array.isArray(list) ? list : []).forEach((item, index) => {
          out += expand(body, Object.assign(Object.create(scope), { [as]: item, index }));
        });
      } else {
        const v = lookup(scope, holePath(attr(m[2], 'value')));
        if (v) out += expand(body, scope);
      }
      i = bodyEnd + `</${m[1]}>`.length;
    }
    return out + interpolate(src.slice(i), scope);
  }

  function render(source) {
    const scriptM = /<script[^>]*type="text\/x-dc"[^>]*>([\s\S]*?)<\/script>/.exec(source);
    const propsM = /<script[^>]*type="text\/x-dc"[^>]*data-props='([^']*)'/.exec(source);
    const tplM = /<x-dc>([\s\S]*?)<\/x-dc>/.exec(source);
    if (!scriptM || !tplM) throw new Error('not a .dc.html design component');

    const Component = new Function('DCLogic', `${scriptM[1]}\nreturn Component;`)(DCLogic);
    const props = propsM ? JSON.parse(propsM[1]) : {};
    const vals = new Component(props).renderVals();

    let html = expand(tplM[1], vals);

    // <helmet> → <head>. Remote font links are dropped (the harness injects local @font-face rules instead).
    const helmetM = /<helmet>([\s\S]*?)<\/helmet>/.exec(html);
    if (helmetM) {
      html = html.replace(helmetM[0], '');
      const tmp = document.createElement('div');
      tmp.innerHTML = helmetM[1];
      for (const el of Array.from(tmp.children)) {
        if (el.tagName === 'LINK' && /fonts\.(googleapis|gstatic)\.com/.test(el.getAttribute('href') || '')) continue;
        document.head.appendChild(el);
      }
    }
    document.body.innerHTML = html;
    document.documentElement.setAttribute('data-dc-rendered', 'true');
    return props;
  }

  window.DCLogic = DCLogic;
  window.__renderDc = render;
  document.addEventListener('DOMContentLoaded', () => {
    try {
      render(window.__DC_SOURCE__);
    } catch (e) {
      document.body.textContent = 'dc-shim: ' + e.message;
      document.documentElement.setAttribute('data-dc-error', e.message);
    }
  });
})();
