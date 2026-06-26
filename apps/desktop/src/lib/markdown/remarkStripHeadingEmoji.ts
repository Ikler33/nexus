import type { Heading, Root, Text } from 'mdast';
import { visit } from 'unist-util-visit';

import { removeHeadingEmoji } from '../editor/headingText';

/**
 * remark-плагин (Hermes-8 фикс): убирает эмодзи из ТЕКСТА заголовков (H1–H6) в режиме чтения —
 * шаблон daily даёт `## 🧠 …`/`## 💡 …`, владелец хочет чистый редакционный вид (Cormorant) без
 * эмодзи. Работает на mdast-уровне. Ставится РАНО — ДО `remarkNexus` (тот сплитит `[[wikilink]]`/`#tag`
 * внутри заголовка в link-узлы): так эмодзи срезаются с ещё цельных text-узлов, а вики/теги не страдают.
 * Внутрь code/inlineCode НЕ лезем (там не `text`).
 *
 * adversarial FIX 1 (CRITICAL): заголовок может состоять из НЕСКОЛЬКИХ inline-узлов
 * (`## Раздел **A** и B` = text('Раздел ') + strong('A') + text(' и B')). Поузельный `.trim()` стёр бы
 * граничные пробелы МЕЖДУ узлами → `РазделAи B` (регрессия для любого bold/italic/code/вики-заголовка,
 * даже без эмодзи). Поэтому:
 *  1) для КАЖДОГО text-ребёнка — только `removeHeadingEmoji` (вырезает эмодзи, БЕЗ trim/collapse границ);
 *  2) collapse сдвоенных пробелов — ВНУТРИ одного узла (безопасно, между узлами НЕ трогаем);
 *  3) trim границ — на уровне всего заголовка: leading-trim ПЕРВОГО text-узла + trailing-trim ПОСЛЕДНЕГО
 *     (ведущий «📅 » и хвостовой эмодзи-пробел уходят, внутренние стыки слов целы).
 *
 * Это display-трансформа — исходный `.md` не мутируется (правка только в AST-памяти рендера). slug
 * заголовка (HEADANCHOR-1) считается ПОСЛЕ этого плагина из очищенного текста → якоря без эмодзи,
 * jump по `data-outline-line` (а не по тексту) не страдает. Эмодзи ТОЛЬКО в заголовках: тело абзацев,
 * приоритеты задач (PRIO_EMOJI), callout-иконки и теги не затрагиваются (этот visit бьёт лишь heading).
 */
export function remarkStripHeadingEmoji() {
  return (tree: Root): void => {
    visit(tree, 'heading', (heading: Heading) => {
      // text-дети заголовка В ПОРЯДКЕ ДОКУМЕНТА (visit — pre-order DFS). Очищаем эмодзи поузельно + collapse
      // двойных пробелов ВНУТРИ узла; границы трогаем только у первого/последнего узла ниже.
      const texts: Text[] = [];
      visit(heading, 'text', (node: Text) => {
        node.value = removeHeadingEmoji(node.value).replace(/ {2,}/g, ' ');
        texts.push(node);
      });
      if (texts.length === 0) return;
      // Границы заголовка: ведущий пробел (от срезанного «📅 ») у ПЕРВОГО text-узла и хвостовой у ПОСЛЕДНЕГО.
      // Внутренние стыки слов между узлами НЕ трогаем (FIX 1). Первый и последний могут совпадать (1 узел).
      texts[0].value = texts[0].value.replace(/^\s+/, '');
      texts[texts.length - 1].value = texts[texts.length - 1].value.replace(/\s+$/, '');
    });
  };
}
