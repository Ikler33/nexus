import type { PhrasingContent, Root, Strong, Text } from 'mdast';
import { visit } from 'unist-util-visit';

/**
 * remark-плагин: Obsidian-выделение `==текст==` в read-only preview (Live-Preview). Не CommonMark —
 * на mdast-уровне residual `text`-узлы (внутрь inlineCode/code-fence не лезем, как [[wikilink]]/#tag).
 * Эмитит нативный `<mark>` (data.hName на узле — без сырого HTML, CSP не трогаем). Ставится ДО
 * remarkNexus, чтобы `==[[Note]]==` дал mark с вложенной вики-ссылкой (remarkNexus добивает text внутри).
 */

// `==…==`: одна строка, без `=` внутри (исключает `===`/`====` и `==a=b==`). ОДИН ленивый квантор +
// литерал `==` → линейно (двойной `*?…*?` давал O(n²) на длинной строке без закрывашки — ReDoS-класс,
// находка ревью). Пустое/пробельное содержимое отсеиваем пост-фильтром (`==  ==` не выделение).
const HIGHLIGHT_RE = /==([^=\n]+?)==/g;

function markNode(value: string): Strong {
  // Несём `<mark>` на Strong-узле через data.hName (mdast-util-to-hast рендерит <mark>, не <strong>).
  return { type: 'strong', data: { hName: 'mark' }, children: [{ type: 'text', value }] };
}

/** Разбивает строку на `text`/`mark` по `==…==`. Чистая — тестируется отдельно. */
export function splitHighlights(value: string): PhrasingContent[] {
  const out: PhrasingContent[] = [];
  let last = 0;
  const re = new RegExp(HIGHLIGHT_RE.source, HIGHLIGHT_RE.flags);
  let m: RegExpExecArray | null;
  while ((m = re.exec(value)) !== null) {
    if (m[1].trim() === '') continue; // `==  ==` — не выделение; оставляем литералом (не двигаем last)
    if (m.index > last) out.push({ type: 'text', value: value.slice(last, m.index) });
    out.push(markNode(m[1]));
    last = re.lastIndex;
  }
  if (out.length === 0) return [{ type: 'text', value }];
  if (last < value.length) out.push({ type: 'text', value: value.slice(last) });
  return out;
}

export function remarkHighlight() {
  return (tree: Root): void => {
    visit(tree, 'text', (node: Text, index, parent) => {
      if (index == null || !parent) return;
      if (!node.value.includes('==')) return; // быстрый отсев
      const parts = splitHighlights(node.value);
      if (parts.length === 1 && parts[0].type === 'text') return; // совпадений нет
      parent.children.splice(index, 1, ...parts);
      return index + parts.length; // продолжить обход после вставленных узлов
    });
  };
}
