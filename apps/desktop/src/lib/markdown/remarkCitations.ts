import type { Link, PhrasingContent, Root, Text } from 'mdast';
import { visit } from 'unist-util-visit';

/**
 * remark-плагин (AIP-2): превращает текстовые цитаты-сноски `[n]` в ответе ИИ в `link`-узлы кастомной
 * схемы `nexus-cite:n`, которые `ChatView` рендерит кликабельной цитатой (клик → открыть источник n:
 * заметку RAG или web-URL). Работает на mdast-уровне → внутрь inline-code/code-fence НЕ лезет (там
 * `inlineCode`/`code`-узлы, а не `text`). Markdown-ссылки `[1](url)` и ref-ссылки `[1]: …` тоже не
 * затрагиваются (они — `link`/`linkReference`-узлы, не `text`). Неизвестный номер (вне диапазона
 * источников) ChatView отрисует обычным текстом — плагин лишь размечает кандидатов.
 */

/** Кастомная URL-схема узла-цитаты (распознаётся в `ChatView` по префиксу href). */
export const CITE_SCHEME = 'nexus-cite:';

/** `[12]` — 1–3 цифры в квадратных скобках. */
const RE = /\[(\d{1,3})\]/g;

function citeNode(n: string): Link {
  return { type: 'link', url: CITE_SCHEME + n, title: null, children: [{ type: 'text', value: `[${n}]` }] };
}

/**
 * Разбивает строку на узлы `text`/`link` по сноскам `[n]`. Чистая — тестируется отдельно.
 * Если ничего не найдено — вернёт один text-узел с исходным значением.
 */
export function splitCitations(value: string): PhrasingContent[] {
  const out: PhrasingContent[] = [];
  let last = 0;
  const re = new RegExp(RE.source, RE.flags);
  let m: RegExpExecArray | null;
  while ((m = re.exec(value)) !== null) {
    if (m.index > last) out.push({ type: 'text', value: value.slice(last, m.index) });
    out.push(citeNode(m[1]));
    last = re.lastIndex;
  }
  if (out.length === 0) return [{ type: 'text', value }];
  if (last < value.length) out.push({ type: 'text', value: value.slice(last) });
  return out;
}

/** remark-плагин: заменяет сноски `[n]` в text-узлах на link-узлы схемы `nexus-cite:`. */
export function remarkCitations() {
  return (tree: Root): void => {
    visit(tree, 'text', (node: Text, index, parent) => {
      if (index == null || !parent) return;
      if (!node.value.includes('[')) return; // быстрый отсев
      const parts = splitCitations(node.value);
      if (parts.length === 1 && parts[0].type === 'text') return; // совпадений нет
      parent.children.splice(index, 1, ...parts);
      return index + parts.length; // продолжить обход после вставленных узлов
    });
  };
}
