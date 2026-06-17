import type { Paragraph, Parent, Root, Text } from 'mdast';
import { visit } from 'unist-util-visit';

/**
 * remark-плагин: Obsidian-комментарии `%%…%%` скрываются в режиме чтения (Live-Preview). Вырезаем на
 * mdast-уровне (text-узлы) — внутрь inlineCode/code-fence НЕ лезем (там не text), как [[wikilink]]/#tag.
 * Покрывает инлайн `%%c%%` и блок `%%\n…\n%%` в пределах ОДНОГО абзаца (мягкие переносы). Неполный `%%`
 * без пары — остаётся литералом (не съедает остаток документа: не жадный + закрывашка обязательна).
 * Комментарий просто УДАЛЯЕТСЯ из дерева → ни рендера, ни HTML/санитайз-поверхности. Ставится ПЕРВЫМ,
 * чтобы закомментированные `[[ссылки]]`/`#теги`/callout-маркеры внутри `%%` не успели обработаться.
 *
 * Граница: блок-коммент через ПУСТУЮ строку (отдельные абзацы) не покрыт — text-визит не пересекает
 * границы абзацев; такой синтаксис в Obsidian редок (обычный блок-коммент — один абзац без пустых строк).
 */
const COMMENT_RE = /%%[\s\S]*?%%/g;

export function remarkComments() {
  return (tree: Root): void => {
    // 1) Вырезаем %%…%% внутри text-узлов; опустевший узел удаляем.
    visit(tree, 'text', (node: Text, index, parent: Parent | undefined) => {
      if (index == null || !parent || !node.value.includes('%%')) return;
      const stripped = node.value.replace(COMMENT_RE, '');
      if (stripped === node.value) return; // полных пар нет
      if (stripped.trim() === '') {
        parent.children.splice(index, 1);
        return index; // продолжить с того же индекса (узел удалён)
      }
      node.value = stripped;
    });
    // 2) Подчищаем абзацы, ставшие ПУСТЫМИ (были целиком комментом).
    visit(tree, 'paragraph', (node: Paragraph, index, parent: Parent | undefined) => {
      if (index == null || !parent) return;
      if (node.children.length === 0) {
        parent.children.splice(index, 1);
        return index;
      }
    });
  };
}
