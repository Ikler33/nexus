import type { Root } from 'hast';
import { visit } from 'unist-util-visit';

/**
 * CSP-фикс для rehype-katex (формулы #4). Строгий CSP проекта запрещает инлайн-стили
 * (`style-src 'self'` без `'unsafe-inline'`), а KaTeX в нескольких местах их генерирует НЕЗАВИСИМО от
 * `output:'mathml'`:
 *  - битый LaTeX → `<span class="katex-error" style="color:#cc0000">` (errorColor всегда инлайнится,
 *    опции отключить нет);
 *  - `\fcolorbox{…}` → `<mpadded style="border:…">` (в MathML нет атрибута границы, только инлайн-стиль).
 * Поэтому снимаем инлайн-`style` со ВСЕХ element-узлов hast ПОСЛЕ rehypeKatex. Это безопасно и
 * исчерпывающе: превью не рендерит сырой HTML (rehype-raw отключён) и сам не задаёт инлайн-стилей —
 * единственный их источник тут KaTeX, а под строгим CSP любой инлайн-стиль всё равно был бы заблокирован
 * вебвью. Цвет ошибки задаёт CSS-класс `.katex-error` (MarkdownPreview.module.css). На валидных формулах
 * инлайн-стилей нет — здесь no-op.
 */
export function rehypeKatexCsp() {
  return (tree: Root): void => {
    visit(tree, 'element', (node) => {
      if (node.properties && 'style' in node.properties) {
        delete node.properties.style;
      }
    });
  };
}
