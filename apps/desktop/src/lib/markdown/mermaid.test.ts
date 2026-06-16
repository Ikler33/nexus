import { describe, expect, it } from 'vitest';

import { cspSafeSvg, parseCss } from './mermaid';

describe('parseCss', () => {
  it('плоские правила → селектор + decls', () => {
    expect(parseCss('.node rect{fill:#eee;stroke:#333}')).toEqual([
      { selector: '.node rect', decls: { fill: '#eee', stroke: '#333' } },
    ]);
  });
  it('несколько правил в порядке источника', () => {
    const r = parseCss('.a{fill:red} .b{stroke:blue}');
    expect(r.map((x) => x.selector)).toEqual(['.a', '.b']);
  });
  it('пропускает @media/@keyframes (вложенные {})', () => {
    const r = parseCss('@media print{.x{fill:red}} .y{fill:green} @keyframes k{from{x:0}to{x:1}}');
    expect(r.map((x) => x.selector)).toEqual(['.y']);
  });
  it('срезает комментарии', () => {
    expect(parseCss('/* c */ .z{fill:#000}')).toEqual([{ selector: '.z', decls: { fill: '#000' } }]);
  });
});

describe('cspSafeSvg (CSP-санитайз mermaid-SVG)', () => {
  const SVG =
    `<svg xmlns="http://www.w3.org/2000/svg">` +
    `<style>.node rect{fill:#ECECFF;stroke:#9370DB;stroke-width:1px;background:red}</style>` +
    `<g class="node"><rect x="0" y="0" width="10" height="10"/></g>` +
    `<script>alert(1)</script>` +
    `<rect class="other" style="fill:#abc;opacity:0.5"/>` +
    `<g onclick="evil()"><circle/></g>` +
    `</svg>`;

  it('переносит <style>-правила в presentation-атрибуты', () => {
    const out = cspSafeSvg(SVG);
    const d = new DOMParser().parseFromString(out, 'image/svg+xml');
    const rect = d.querySelector('g.node rect');
    expect(rect?.getAttribute('fill')).toBe('#ECECFF');
    expect(rect?.getAttribute('stroke')).toBe('#9370DB');
    expect(rect?.getAttribute('stroke-width')).toBe('1px');
  });

  it('НЕ переносит не-presentation CSS-свойства (background)', () => {
    const out = cspSafeSvg(SVG);
    const d = new DOMParser().parseFromString(out, 'image/svg+xml');
    expect(d.querySelector('g.node rect')?.getAttribute('background')).toBeNull();
  });

  it('inline-style= → presentation-атрибуты, сам style= удалён', () => {
    const out = cspSafeSvg(SVG);
    const d = new DOMParser().parseFromString(out, 'image/svg+xml');
    const other = d.querySelector('rect.other');
    expect(other?.getAttribute('fill')).toBe('#abc');
    expect(other?.getAttribute('opacity')).toBe('0.5');
    expect(other?.hasAttribute('style')).toBe(false);
  });

  it('CSP-safe: НЕТ <style>, НЕТ style=, НЕТ <script>, НЕТ on*-обработчиков', () => {
    const out = cspSafeSvg(SVG);
    expect(out).not.toMatch(/<style[\s>]/i);
    expect(out).not.toMatch(/\sstyle=/i);
    expect(out).not.toMatch(/<script/i);
    expect(out).not.toMatch(/onclick=/i);
  });

  it('javascript:-ссылки (href/xlink:href) вырезаются (XSS-гард)', () => {
    const svg =
      `<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink">` +
      `<a href="javascript:alert(1)"><text>x</text></a>` +
      `<a xlink:href="JavaScript:evil()"><text>y</text></a>` +
      `<a href="https://ok.example"><text>z</text></a></svg>`;
    const out = cspSafeSvg(svg);
    expect(out).not.toMatch(/javascript:/i);
    // безопасный https-href сохраняется
    expect(out).toMatch(/https:\/\/ok\.example/);
  });

  it('SMIL-анимации (<animate>/<set>/…) вырезаются (могут динамически задать опасные атрибуты)', () => {
    const svg =
      `<svg xmlns="http://www.w3.org/2000/svg"><rect>` +
      `<set attributeName="onclick" to="evil()"/>` +
      `<animate attributeName="href" to="javascript:x"/>` +
      `</rect></svg>`;
    const out = cspSafeSvg(svg);
    expect(out).not.toMatch(/<set|<animate/i);
  });

  it('опасные/внешние ссылки режутся по схеме (data:/vbscript:/внешний use); безопасные сохранены', () => {
    const svg =
      `<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink">` +
      `<a href="data:text/html,evil"><text>a</text></a>` +
      `<a href="vbscript:msgbox"><text>b</text></a>` +
      `<a href="https://ok.example"><text>c</text></a>` +
      `<use xlink:href="https://evil.example/x.svg#g"/>` +
      `<use href="#localGroup"/>` +
      `</svg>`;
    const out = cspSafeSvg(svg);
    expect(out).not.toMatch(/data:text\/html/i);
    expect(out).not.toMatch(/vbscript:/i);
    expect(out).not.toMatch(/evil\.example/i); // внешний <use> — вырезан
    expect(out).toMatch(/https:\/\/ok\.example/); // http(s) на <a> — сохранён
    expect(out).toMatch(/#localGroup/); // внутренний <use> — сохранён
  });

  it('!important у mermaid-classDef сохраняет цвет (как валидный presentation-атрибут)', () => {
    const svg =
      `<svg xmlns="http://www.w3.org/2000/svg"><style>.cls > * { fill:#f00 !important }</style>` +
      `<g class="cls"><rect/></g></svg>`;
    const out = cspSafeSvg(svg);
    const d = new DOMParser().parseFromString(out, 'image/svg+xml');
    expect(d.querySelector('g.cls rect')?.getAttribute('fill')).toBe('#f00');
  });

  it('невалидный вход → пустая строка', () => {
    expect(cspSafeSvg('не svg вовсе')).toBe('');
    expect(cspSafeSvg('<div>html</div>')).toBe('');
  });
});
