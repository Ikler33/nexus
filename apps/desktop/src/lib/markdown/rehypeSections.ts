import type { Element, ElementContent, Root, RootContent } from 'hast';

import { slugify } from '../editor/slug';

/**
 * Группировка тела в сворачиваемые H2-секции (Hermes-8 S3 «Редакция»). ReactMarkdown отдаёт ПЛОСКИЙ
 * поток узлов (h2, p, ul, h2, …) — а для номеров секций «01/02», шеврона и СВОРАЧИВАНИЯ нужна
 * иерархия. Этот плагин (последний в rehype-цепочке, видит полный hast) оборачивает каждый top-level
 * `h2` и его сиблинги ДО следующего `h2` (или конца) в:
 *
 *   <section class="sec" data-sec-id="<slug>">
 *     <h2>…</h2>            ← САМ узел h2 (перемещён, НЕ скопирован)
 *     <div class="sec-body"><div class="sec-inner">…остальные узлы секции…</div></div>
 *   </section>
 *
 * Двойная обёртка (`sec-body` > `sec-inner`) — для grid-rows-анимации сворачивания: `.sec-body` —
 * grid c ОДНИМ ребёнком (`grid-template-rows:1fr→0fr`), `.sec-inner` — `overflow:hidden; min-height:0`.
 * Так развёрнутая секция = натуральная высота БЕЗ потолка (длинные таблицы/код/проза не обрезаются —
 * прежний `max-height:1400px` резал контент ниже фолда), а свёрнутая плавно уходит в 0.
 *
 * ИНВАРИАНТЫ:
 *  - HEADANCHOR-1: узел h2 ПЕРЕМЕЩАЕТСЯ as-is (не мутируется и не клонируется) → React-оверрайд
 *    `components.h2` по-прежнему срабатывает и проставляет `id`(slug)/`data-outline-line`. Панель
 *    Outline, scroll-spy, `#heading`-якоря, сноски не ломаются.
 *  - Лид/интро (контент ДО первого h2) НЕ оборачивается — рендерится плоско (не сворачивается).
 *  - Без-H2 документ → секций нет, поток отдаётся как есть (нет обёрток, ничего не падает).
 *  - H3 НЕ группируются — только top-level H2 дают секции (H3 живут внутри `.sec-body`).
 *  - GFM-блок сносок (`<section data-footnotes class=footnotes>`) НЕ всасывается в тело последней секции
 *    (иначе спрятался бы при её сворачивании) — он и весь хвост после него выносятся top-level.
 *
 * `data-sec-id` = `slugify(текст h2)` — тот же slug, что у React-оверрайда заголовков → стабильный
 * ключ состояния сворачивания, переживающий правки в ДРУГИХ секциях (не позиционный индекс). Дубликаты
 * имён секций дедуплицируются суффиксом `-1`, `-2` (как slugger заголовков), чтобы тоггл одной секции
 * не схлопывал одноимённую соседнюю.
 */
export function rehypeSections() {
  return (tree: Root): void => {
    const children = tree.children;
    // Быстрый выход: нет ни одного top-level h2 → плоский документ, обёртки не нужны.
    if (!children.some((n) => isHeading(n, 'h2'))) return;

    const out: RootContent[] = [];
    const seen = new Map<string, number>(); // дедуп одноимённых секций (per-render)
    let i = 0;

    // Лид/интро: всё ДО первого h2 — оставляем как есть, вне секций.
    while (i < children.length && !isHeading(children[i], 'h2')) {
      out.push(children[i]);
      i += 1;
    }

    // Каждую группу h2 + сиблинги-до-следующего-h2 → <section>. Набор тела ОСТАНАВЛИВАЕТСЯ и на
    // GFM-блоке сносок: он и весь хвост после него выносятся top-level (вне секций → видны при сворачивании).
    let tailStart = children.length; // индекс, с которого хвост (footnotes+далее) уходит наружу
    while (i < children.length) {
      const h2 = children[i] as Element; // гарантированно h2 по условию цикла
      i += 1;
      const bodyNodes: ElementContent[] = [];
      while (i < children.length && !isHeading(children[i], 'h2') && !isFootnotes(children[i])) {
        bodyNodes.push(children[i] as ElementContent);
        i += 1;
      }

      const secId = dedupSlug(seen, hastText(h2));
      // Метим САМ h2 тем же secId (НЕ копируя узел) — `components.h2` по нему понимает, что заголовок
      // внутри секции, и берёт ИМЕННО этот id (одна точка дедупа). Так состояние сворачивания на h2 и на
      // `<section>` всегда совпадает; recompute slug в React-оверрайде (со своим slugger + leadSlug под
      // masthead) дал бы расхождение. id/data-outline-line h2 проставит React-оверрайд (HEADANCHOR-1) —
      // мы его НЕ трогаем, только добавляем data-атрибут в properties.
      h2.properties = { ...(h2.properties ?? {}), 'data-sec-id': secId };
      // Внутренняя обёртка sec-inner — единственный ребёнок sec-body (grid-rows-анимация без потолка высоты).
      const secInner: Element = {
        type: 'element',
        tagName: 'div',
        properties: { className: ['sec-inner'] },
        children: bodyNodes,
      };
      const secBody: Element = {
        type: 'element',
        tagName: 'div',
        properties: { className: ['sec-body'] },
        children: [secInner],
      };
      const section: Element = {
        type: 'element',
        tagName: 'section',
        properties: { className: ['sec'], 'data-sec-id': secId },
        children: [h2 as ElementContent, secBody], // h2 перемещён первым ребёнком, не скопирован
      };
      out.push(section);

      // Уперлись в footnotes-секцию → дальше секции не нарезаем: хвост целиком наружу.
      if (i < children.length && isFootnotes(children[i])) {
        tailStart = i;
        break;
      }
    }

    // Хвост после последней секции (footnotes-блок и всё за ним) — top-level, вне секций.
    for (let j = tailStart; j < children.length; j += 1) out.push(children[j]);

    tree.children = out;
  };
}

/** Узел — element заданного tagName (h2/…). Текстовые/комментарии/doctype отсекаются. */
function isHeading(node: RootContent | undefined, tag: string): node is Element {
  return node?.type === 'element' && (node as Element).tagName === tag;
}

/** GFM-блок сносок: `<section class="footnotes" data-footnotes>` — НЕ всасываем его в тело секции. */
function isFootnotes(node: RootContent | undefined): node is Element {
  if (node?.type !== 'element') return false;
  const el = node as Element;
  if (el.tagName !== 'section') return false;
  if ('dataFootnotes' in (el.properties ?? {})) return true;
  const cls = el.properties?.className;
  return Array.isArray(cls) && cls.includes('footnotes');
}

/** Плоский текст hast-узла (для slug секции) — рекурсивно собираем `value` text-узлов, как у заголовков. */
function hastText(node: ElementContent): string {
  if (node.type === 'text') return node.value;
  if (node.type === 'element') return node.children.map(hastText).join('');
  return '';
}

/** Дедуп slug в пределах документа: повтор → `slug-1`, `slug-2`. Пустой → `section` (как slugger). */
function dedupSlug(seen: Map<string, number>, text: string): string {
  const base = slugify(text) || 'section';
  const n = seen.get(base) ?? 0;
  seen.set(base, n + 1);
  return n === 0 ? base : `${base}-${n}`;
}
