import type { Blockquote, Paragraph, PhrasingContent, Root, Text } from 'mdast';
import { visit } from 'unist-util-visit';

/**
 * remark-плагин: Obsidian-callouts (admonitions) `> [!note] Заголовок` в read-only preview
 * (Live-Preview). Работает на mdast-уровне — берёт `blockquote`, у которого ПЕРВАЯ строка первого
 * абзаца начинается с маркера `[!тип]` (+/- — сворачивание), и перерисовывает его в кастомный узел
 * `nexus-callout` (тот же приём data.hName/hProperties, что у [[remarkEmbeds]]). CSP-безопасно:
 * никакого сырого HTML — `MarkdownPreview` рендерит `nexus-callout` своим компонентом, иконки —
 * инлайновый SVG (lucide), цвета/тинт — классами и `data-callout`-селекторами (без inline-style).
 * Не-callout цитаты не трогаем (ранний выход). Внутрь code-fence/inline-code не лезем (там не text).
 */

export type CalloutFold = '' | '+' | '-';

export interface CalloutMarker {
  /** Тип в нижнем регистре, как написан (`note`, `warning`, `tip`, алиасы — нормализуются в компоненте). */
  kind: string;
  /** Сворачивание: '' — не сворачиваемый, '-' — свёрнут по умолчанию, '+' — развёрнут, но сворачиваемый. */
  fold: CalloutFold;
  /** Тип ровно как написан (для дефолтной подписи Title-case, когда заголовок не задан). */
  rawLabel: string;
}

// Маркер в НАЧАЛЕ первой строки: `[!type]`, опц. `+`/`-`, опц. пробелы, далее — заголовок.
// Тип: буква + [\w-]* (как Obsidian). Совпадение только в начале text-узла (mid-paragraph `[!x]` — не callout).
const MARKER_RE = /^\[!([A-Za-z][\w-]*)\]([+-]?)[ \t]*/;

/** Разбирает маркер callout из значения первого text-узла. null — не callout. Чистая (тестируется). */
export function parseCalloutMarker(value: string): { marker: CalloutMarker; rest: string } | null {
  const m = MARKER_RE.exec(value);
  if (!m) return null;
  return {
    marker: { kind: m[1].toLowerCase(), fold: (m[2] as CalloutFold) || '', rawLabel: m[1] },
    rest: value.slice(m[0].length),
  };
}

/**
 * Делит inline-содержимое первого абзаца по ПЕРВОМУ переносу строки `\n` (мягкий перенос внутри
 * абзаца). До переноса — заголовок callout, после — начало тела (Obsidian: первая строка = title).
 * Нет `\n` — весь inline это заголовок (body=null, тело берётся из последующих абзацев цитаты).
 * Чистая — тестируется отдельно.
 */
export function splitInlineAtNewline(nodes: PhrasingContent[]): {
  title: PhrasingContent[];
  body: PhrasingContent[] | null;
} {
  for (let i = 0; i < nodes.length; i++) {
    const n = nodes[i];
    // Жёсткий перенос (2+ пробела/обратный слэш на конце строки) remark отдаёт ОТДЕЛЬНЫМ `break`-узлом
    // и «съедает» `\n` (его нет в тексте). Тоже делитель заголовок/тело: до — заголовок, после — тело.
    if (n.type === 'break') {
      const body = nodes.slice(i + 1);
      return { title: nodes.slice(0, i), body: body.length ? body : null };
    }
    if (n.type === 'text' && n.value.includes('\n')) {
      const idx = n.value.indexOf('\n');
      const before = n.value.slice(0, idx);
      const after = n.value.slice(idx + 1);
      const title: PhrasingContent[] = nodes.slice(0, i);
      if (before) title.push({ type: 'text', value: before });
      const body: PhrasingContent[] = [];
      if (after) body.push({ type: 'text', value: after });
      body.push(...nodes.slice(i + 1));
      return { title, body };
    }
  }
  return { title: nodes, body: null };
}

/** mdast-узел с data-полями для mdast-util-to-hast (hName → имя HTML-тега, hProperties → атрибуты). */
type WithHast = { data?: { hName?: string; hProperties?: Record<string, unknown> } };

export function remarkCallouts() {
  return (tree: Root): void => {
    visit(tree, 'blockquote', (node: Blockquote) => {
      const first = node.children[0];
      if (!first || first.type !== 'paragraph') return;
      const firstInline = first.children[0];
      if (!firstInline || firstInline.type !== 'text') return;
      const parsed = parseCalloutMarker(firstInline.value);
      if (!parsed) return;
      const { marker, rest } = parsed;

      // Первый абзац без маркера: остаток первого text-узла (если есть) + прочие inline-узлы абзаца.
      const inline: PhrasingContent[] = rest
        ? [{ type: 'text', value: rest } as Text, ...first.children.slice(1)]
        : first.children.slice(1);

      const { title, body } = splitInlineAtNewline(inline);

      // Тело callout: остаток первой строки (если был перенос) как абзац + остальные абзацы цитаты.
      const bodyChildren = [
        ...(body && body.length ? [{ type: 'paragraph', children: body } as Paragraph] : []),
        ...node.children.slice(1),
      ];

      // Узел-заголовок: абзац, который to-hast отрендерит как <nexus-callout-title> (иконка + подпись).
      const titleNode: Paragraph & WithHast = {
        type: 'paragraph',
        data: { hName: 'nexus-callout-title', hProperties: { kind: marker.kind, label: marker.rawLabel } },
        children: title,
      };

      // Перерисовываем сам blockquote в <nexus-callout>: первый ребёнок — заголовок, далее — тело.
      node.children = [titleNode, ...bodyChildren] as Blockquote['children'];
      const data = ((node as Blockquote & WithHast).data ??= {});
      data.hName = 'nexus-callout';
      data.hProperties = { kind: marker.kind, fold: marker.fold || undefined };
    });
  };
}
