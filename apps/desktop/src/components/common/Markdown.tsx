import ReactMarkdown, { type Components, type Options } from 'react-markdown';
import remarkGfm from 'remark-gfm';

import { MermaidDiagram } from './MermaidDiagram';
import { remarkMermaid } from '../../lib/markdown/remarkMermaid';
import styles from './Markdown.module.css';

/**
 * Переиспользуемый markdown-рендер. LLM отвечает в markdown (фидбэк 11.06: сырые `##`/`**`
 * выглядят плохо) — этот компонент даёт единый вид и для чата, и для вкладки Агента.
 *
 * Безопасность: react-markdown v10 НЕ рендерит сырой HTML по умолчанию (нет `rehype-raw`) — так и
 * оставляем. `remark-gfm` подключён всегда; вызывающий может добавить свои `remarkPlugins`
 * (напр. `remarkCitations` в чате) и переопределить `components`/`urlTransform`.
 *
 * W-35: mermaid по умолчанию ВКЛ (`mermaid` дефолт `true`) — фенс ` ```mermaid ` → ленивый
 * CSP-безопасный SVG (`remarkMermaid` → узел `nexus-mermaid` → `MermaidDiagram`, образец —
 * `MarkdownPreview`). Так агент-финал получает диаграммы автоматически. Для стрима ставим
 * `mermaid={false}` (частичная/недописанная диаграмма мигала бы) — финальный рендер отрисует её целиком.
 *
 * NB: типы `remarkPlugins`/`urlTransform` берём из `Options` самого react-markdown (а не из
 * транзитивного `unified`) — путь импорта стабилен и tsc доволен.
 */
export function Markdown({
  content,
  remarkPlugins = [],
  components,
  urlTransform,
  className,
  mermaid = true,
}: {
  content: string;
  remarkPlugins?: Options['remarkPlugins'];
  components?: Components;
  urlTransform?: Options['urlTransform'];
  className?: string;
  mermaid?: boolean;
}) {
  // mermaid вкл → дорисовываем remarkMermaid + рендер узла `nexus-mermaid`, НЕ затирая то, что передал
  // вызывающий (его plugins/components имеют приоритет — спред `...components` после нашего ключа).
  const plugins: Options['remarkPlugins'] = mermaid
    ? [remarkGfm, remarkMermaid, ...(remarkPlugins ?? [])]
    : [remarkGfm, ...(remarkPlugins ?? [])];
  let mergedComponents: Components | undefined = components;
  if (mermaid) {
    // `nexus-mermaid` — кастомный hast-элемент (не из стандартного набора `Components`); ключуем через
    // cast, как `MarkdownPreview`. Спред `...components` ПОСЛЕ — переопределение вызывающим уважается.
    mergedComponents = {
      ...({
        'nexus-mermaid': ({ node }: { node?: { properties?: Record<string, unknown> } }) => {
          const code = node?.properties?.code;
          return typeof code === 'string' && code.trim() ? <MermaidDiagram code={code} /> : null;
        },
      } as Components),
      ...components,
    };
  }
  return (
    <div className={className ? `${styles.md} ${className}` : styles.md}>
      <ReactMarkdown remarkPlugins={plugins} components={mergedComponents} urlTransform={urlTransform}>
        {content}
      </ReactMarkdown>
    </div>
  );
}
