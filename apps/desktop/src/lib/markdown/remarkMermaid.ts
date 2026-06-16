import type { Code, Root } from 'mdast';
import { visit } from 'unist-util-visit';

/**
 * remark-плагин: код-фенс ` ```mermaid ` → кастомный узел `nexus-mermaid` (через `data.hName`/
 * `hProperties` → элемент в hast), который `MarkdownPreview` рендерит компонентом `MermaidDiagram`
 * (ленивый рендер mermaid → CSP-безопасный SVG). Прочие фенсы — обычный `<pre><code>` без изменений.
 */
export function remarkMermaid() {
  return (tree: Root): void => {
    visit(tree, 'code', (node: Code, index, parent) => {
      if (index == null || !parent || node.lang?.toLowerCase() !== 'mermaid') return;
      parent.children[index] = {
        type: 'code', // тип игнорируется — рендер по data.hName; держим валидный mdast-узел
        value: '',
        data: { hName: 'nexus-mermaid', hProperties: { code: node.value } },
      } as Code;
    });
  };
}
