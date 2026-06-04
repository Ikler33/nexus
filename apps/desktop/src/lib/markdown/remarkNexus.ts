import type { Link, PhrasingContent, Root, Text } from 'mdast';
import { visit } from 'unist-util-visit';

/**
 * remark-плагин для Nexus-специфики в read-only preview (#20): `[[wikilink]]` и `#tag` (НЕ часть
 * CommonMark) превращаются в `link`-узлы с кастомной URL-схемой, которые `MarkdownPreview` рендерит
 * как кликабельную ссылку / тег-чип. Работает на mdast-уровне → внутрь code-fence/inline-code НЕ лезет
 * (там `code`/`inlineCode`-узлы, а не `text`).
 */

// `[[Target]]`, `[[Target#H|Alias]]` ИЛИ `#tag` с границей (^|\s) — без lookbehind (совместимость WebKit).
const RE = /\[\[([^\]\n]+?)\]\]|(^|\s)(#[\p{L}\d/_-]*\p{L}[\p{L}\d/_-]*)/gu;

/** Кастомные URL-схемы кастомных узлов (распознаются в `MarkdownPreview` по префиксу href). */
export const WIKILINK_SCHEME = 'nexus-wikilink:';
export const TAG_SCHEME = 'nexus-tag:';

/** Цель вики-ссылки: срезает `|alias` и `#heading`. */
export function wikilinkTarget(inner: string): string {
  return inner.split('|')[0].split('#')[0].trim();
}
/** Отображаемая подпись вики-ссылки: alias (после `|`) либо цель без `#heading`. */
function wikilinkLabel(inner: string): string {
  const bar = inner.indexOf('|');
  if (bar >= 0) return inner.slice(bar + 1).trim();
  return inner.split('#')[0].trim();
}

function linkNode(url: string, text: string): Link {
  return { type: 'link', url, title: null, children: [{ type: 'text', value: text }] };
}

/**
 * Разбивает строку на узлы `text`/`link` по `[[wikilink]]` и `#tag`. Чистая — тестируется отдельно.
 * Если ничего не найдено — вернёт один text-узел с исходным значением.
 */
export function splitWikilinksTags(value: string): PhrasingContent[] {
  const out: PhrasingContent[] = [];
  let last = 0;
  const re = new RegExp(RE.source, RE.flags);
  let m: RegExpExecArray | null;
  while ((m = re.exec(value)) !== null) {
    if (m.index > last) out.push({ type: 'text', value: value.slice(last, m.index) });
    if (m[1] !== undefined) {
      // Цель кодируется в URL (пробелы/спецсимволы → валидный href; react-markdown иначе дропнет).
      const target = wikilinkTarget(m[1]);
      out.push(linkNode(WIKILINK_SCHEME + encodeURIComponent(target), wikilinkLabel(m[1]) || target));
    } else {
      if (m[2]) out.push({ type: 'text', value: m[2] }); // граница (^|\s) — вернуть как текст
      out.push(linkNode(TAG_SCHEME + encodeURIComponent(m[3].slice(1)), m[3]));
    }
    last = re.lastIndex;
  }
  if (out.length === 0) return [{ type: 'text', value }];
  if (last < value.length) out.push({ type: 'text', value: value.slice(last) });
  return out;
}

/** remark-плагин: заменяет `[[wikilink]]`/`#tag` в text-узлах на link-узлы кастомной схемы. */
export function remarkNexus() {
  return (tree: Root): void => {
    visit(tree, 'text', (node: Text, index, parent) => {
      if (index == null || !parent) return;
      if (!node.value.includes('[[') && !node.value.includes('#')) return; // быстрый отсев
      const parts = splitWikilinksTags(node.value);
      if (parts.length === 1 && parts[0].type === 'text') return; // совпадений нет
      parent.children.splice(index, 1, ...parts);
      return index + parts.length; // продолжить обход после вставленных узлов
    });
  };
}
