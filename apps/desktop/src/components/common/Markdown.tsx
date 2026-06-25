import ReactMarkdown, { type Components, type Options } from 'react-markdown';
import remarkGfm from 'remark-gfm';

import styles from './Markdown.module.css';

/**
 * Переиспользуемый markdown-рендер. LLM отвечает в markdown (фидбэк 11.06: сырые `##`/`**`
 * выглядят плохо) — этот компонент даёт единый вид и для чата, и для вкладки Агента.
 *
 * Безопасность: react-markdown v10 НЕ рендерит сырой HTML по умолчанию (нет `rehype-raw`) — так и
 * оставляем. `remark-gfm` подключён всегда; вызывающий может добавить свои `remarkPlugins`
 * (напр. `remarkCitations` в чате) и переопределить `components`/`urlTransform`.
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
}: {
  content: string;
  remarkPlugins?: Options['remarkPlugins'];
  components?: Components;
  urlTransform?: Options['urlTransform'];
  className?: string;
}) {
  return (
    <div className={className ? `${styles.md} ${className}` : styles.md}>
      <ReactMarkdown
        remarkPlugins={[remarkGfm, ...(remarkPlugins ?? [])]}
        components={components}
        urlTransform={urlTransform}
      >
        {content}
      </ReactMarkdown>
    </div>
  );
}
