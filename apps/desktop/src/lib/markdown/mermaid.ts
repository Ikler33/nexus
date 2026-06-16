/**
 * Mermaid-диаграммы в режиме чтения под СТРОГИМ CSP (`style-src 'self'`, без unsafe-inline). Подход —
 * **SVG-санитайз без ослабления CSP** (выбор владельца): mermaid рендерит SVG со встроенным `<style>`
 * (CSP-блок) и иногда inline-`style=`; мы парсим эти стили и **переносим в SVG presentation-атрибуты**
 * (`fill`/`stroke`/`font-*` — это АТРИБУТЫ, не подчиняются `style-src`), затем удаляем `<style>`/`style=`/
 * `<script>`/`<foreignObject>`/`on*`. Итог: полностью стилизованный SVG только на presentation-атрибутах →
 * рендерится под строгим CSP и тестируемо НЕ содержит `<style>`/`style=`. Тяжёлый mermaid грузится лениво.
 */

/** CSS-свойства, имеющие эквивалент SVG presentation-атрибута (только их безопасно перенести в атрибут). */
const PRESENTATION_ATTRS = new Set([
  'fill',
  'fill-opacity',
  'fill-rule',
  'stroke',
  'stroke-width',
  'stroke-opacity',
  'stroke-dasharray',
  'stroke-dashoffset',
  'stroke-linecap',
  'stroke-linejoin',
  'stroke-miterlimit',
  'opacity',
  'color',
  'font-family',
  'font-size',
  'font-weight',
  'font-style',
  'text-anchor',
  'dominant-baseline',
  'letter-spacing',
  'word-spacing',
  'visibility',
  'display',
  'cursor',
  'marker-end',
  'marker-start',
  'marker-mid',
]);

interface CssRule {
  selector: string;
  decls: Record<string, string>;
}

/** Разбор `prop: val; …` в карту (общий для `<style>`-правил и inline-`style=`). Срез завершающего
 *  `!important` (ревью: mermaid `classDef` эмитит `fill:#f00 !important` → как presentation-атрибут это
 *  невалидный paint, цвет терялся бы). */
function parseDecls(declStr: string): Record<string, string> {
  const out: Record<string, string> = {};
  for (const part of declStr.split(';')) {
    const c = part.indexOf(':');
    if (c < 0) continue;
    const prop = part.slice(0, c).trim().toLowerCase();
    const val = part
      .slice(c + 1)
      .replace(/!important\s*$/i, '')
      .trim();
    if (prop && val) out[prop] = val;
  }
  return out;
}

/** Безопасна ли ссылка для вставки (ревью XSS). `internalOnly` (`<use>`/`<image>`) — только `#`-якорь
 *  (без внешней загрузки/SSRF). Иначе (`<a>`) — `#`/относительный/http(s)/mailto; режем
 *  `javascript:`/`data:`/`vbscript:`/protocol-relative `//host`/любую прочую схему. */
function safeHref(value: string, internalOnly: boolean): boolean {
  const t = value.trim();
  if (t === '' || t.startsWith('#')) return true;
  if (t.startsWith('//')) return false; // protocol-relative = внешний
  const scheme = /^([a-zA-Z][a-zA-Z0-9+.-]*):/.exec(t);
  if (!scheme) return !internalOnly; // относительный путь: ок для <a>, не для use/image
  if (internalOnly) return false; // use/image со схемой — внешняя загрузка
  return /^(?:https?|mailto)$/i.test(scheme[1]); // <a>: только http(s)/mailto
}

/**
 * Лёгкий парсер CSS из mermaid-`<style>`: плоские правила `selector { decls }`, в порядке источника
 * (поздние перекрывают ранние — приближение каскада). `@media`/`@keyframes` и т.п. — пропускаются
 * (вложенные `{}`; для статической диаграммы анимации/медиа неважны). Чистый — тестируется отдельно.
 */
export function parseCss(css: string): CssRule[] {
  const clean = css.replace(/\/\*[\s\S]*?\*\//g, ''); // срез комментариев
  const rules: CssRule[] = [];
  let i = 0;
  while (i < clean.length) {
    const brace = clean.indexOf('{', i);
    if (brace < 0) break;
    const at = clean.indexOf('@', i);
    if (at >= 0 && at < brace) {
      // @-правило: пропускаем сбалансированный блок `{…}`.
      let depth = 0;
      let j = clean.indexOf('{', at);
      if (j < 0) break;
      for (; j < clean.length; j++) {
        if (clean[j] === '{') depth++;
        else if (clean[j] === '}') {
          depth--;
          if (depth === 0) {
            j++;
            break;
          }
        }
      }
      i = j;
      continue;
    }
    const selector = clean.slice(i, brace).trim();
    const end = clean.indexOf('}', brace);
    if (end < 0) break;
    const decls = parseDecls(clean.slice(brace + 1, end));
    if (selector && Object.keys(decls).length > 0) rules.push({ selector, decls });
    i = end + 1;
  }
  return rules;
}

/** Применяет decls к элементу как presentation-атрибуты (только whitelist; перекрывает существующие —
 *  у CSS-правил приоритет над presentation-атрибутами, у inline-style — над правилами). */
function applyDecls(el: Element, decls: Record<string, string>): void {
  for (const [prop, val] of Object.entries(decls)) {
    if (PRESENTATION_ATTRS.has(prop)) el.setAttribute(prop, val);
  }
}

/**
 * Делает SVG CSP-безопасным: переносит стили (`<style>` + inline) в presentation-атрибуты, удаляет
 * `<style>`/`style=`/`<script>`/`<foreignObject>`/`on*`. Возвращает сериализованный SVG, либо '' если
 * вход не распарсился как SVG. Чистый (строка→строка) — тестируется на фикстуре.
 */
export function cspSafeSvg(svg: string): string {
  const doc = new DOMParser().parseFromString(svg, 'image/svg+xml');
  if (doc.querySelector('parsererror') || doc.documentElement.nodeName.toLowerCase() !== 'svg') {
    return '';
  }
  // 1) `<style>`-правила → presentation-атрибуты (в порядке источника, поздние перекрывают).
  const styleEls = Array.from(doc.querySelectorAll('style'));
  const css = styleEls.map((s) => s.textContent ?? '').join('\n');
  for (const rule of parseCss(css)) {
    let matched: Element[];
    try {
      matched = Array.from(doc.querySelectorAll(rule.selector));
    } catch {
      continue; // невалидный/неподдержимый селектор (`:hover` и т.п.) — пропускаем
    }
    for (const el of matched) applyDecls(el, rule.decls);
  }
  // 2) inline-`style=` → presentation-атрибуты (высший приоритет), затем убрать атрибут.
  for (const el of Array.from(doc.querySelectorAll('[style]'))) {
    applyDecls(el, parseDecls(el.getAttribute('style') ?? ''));
    el.removeAttribute('style');
  }
  // 3) убрать `<style>` (CSP-блок), активный контент, SMIL-анимации и опасные/внешние ссылки. SMIL
  //    (`<animate>`/`<set>`/…) может ДИНАМИЧЕСКИ задать `on*`/опасный href — в статической mermaid-
  //    диаграмме их нет, режем (defense-in-depth; mermaid securityLevel:'strict' их и не создаёт).
  styleEls.forEach((s) => s.remove());
  doc
    .querySelectorAll('script, foreignObject, animate, animateTransform, animateMotion, set')
    .forEach((e) => e.remove());
  for (const el of Array.from(doc.querySelectorAll('*'))) {
    const internalOnly = el.nodeName.toLowerCase() === 'use' || el.nodeName.toLowerCase() === 'image';
    for (const attr of Array.from(el.attributes)) {
      const name = attr.name.toLowerCase();
      const isHref = name === 'href' || name.endsWith(':href');
      if (/^on/.test(name) || (isHref && !safeHref(attr.value, internalOnly))) {
        el.removeAttribute(attr.name);
      }
    }
  }
  return new XMLSerializer().serializeToString(doc.documentElement);
}

/** Тема mermaid: светлая (`default`) или тёмная — маппится из темы приложения в `MermaidDiagram`. */
export type MermaidTheme = 'default' | 'dark';

let mermaidLoad: Promise<typeof import('mermaid').default> | null = null;
let appliedTheme: MermaidTheme | '' = '';

/** Ленивая загрузка mermaid (тяжёлый чанк) + (ре)инициализация под текущую тему. `htmlLabels:false` →
 *  `<text>` вместо `<foreignObject>`; `securityLevel:'strict'` → mermaid сам режет скрипты/интерактив. */
async function getMermaid(theme: MermaidTheme): Promise<typeof import('mermaid').default> {
  if (!mermaidLoad) mermaidLoad = import('mermaid').then((m) => m.default);
  const mermaid = await mermaidLoad;
  if (appliedTheme !== theme) {
    mermaid.initialize({
      startOnLoad: false,
      securityLevel: 'strict',
      htmlLabels: false,
      flowchart: { htmlLabels: false },
      theme,
    });
    appliedTheme = theme;
  }
  return mermaid;
}

/**
 * Рендерит mermaid-код в CSP-безопасный SVG-строку под заданной темой. Бросает при синтаксической ошибке
 * диаграммы (компонент ловит и показывает заглушку). `id` — уникальный (mermaid требует для defs/маркеров).
 */
export async function renderMermaid(
  code: string,
  id: string,
  theme: MermaidTheme = 'default',
): Promise<string> {
  const mermaid = await getMermaid(theme);
  const { svg } = await mermaid.render(id, code);
  const safe = cspSafeSvg(svg);
  if (!safe) throw new Error('mermaid: SVG не распарсился');
  return safe;
}
