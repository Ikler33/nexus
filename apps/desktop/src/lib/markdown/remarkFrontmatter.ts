import type { Root } from 'mdast';

import { extractFrontmatter } from './frontmatter';

/**
 * remark-плагин: убирает ведущий frontmatter `---…---` из markdown-рендера БЕЗ сдвига номеров строк
 * тела. Ключевой инвариант (FRONTMATTER-1): НЕ режем исходник (это сместило бы 1-based строки, на
 * которых держатся EDIT-5 тогл-таски и EDIT-7 оглавление), а удаляем top-узлы, целиком лежащие в
 * строках `[1..endLine]` блока. Тело (строки > endLine) сохраняет позиции дословно. Сам frontmatter
 * рендерится отдельно как Properties-таблица (см. `MarkdownPreview`). Источник берём из `String(file)`
 * (тот же приём, что у remarkEmbeds). Нет frontmatter / незакрыт — no-op.
 *
 * Удаляем по ДИАПАЗОНУ СТРОК, а не по типам узлов: без remark-frontmatter ведущий `---\nk: v\n---`
 * разбирается неоднозначно (thematicBreak + setext-заголовок), и точечное удаление было бы хрупким.
 */
export function remarkFrontmatter() {
  return (tree: Root, file: unknown): void => {
    const fm = extractFrontmatter(String(file));
    if (!fm) return;
    tree.children = tree.children.filter((n) => !(n.position && n.position.end.line <= fm.endLine));
  };
}
