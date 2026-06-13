import ReactMarkdown from 'react-markdown';
import type { Components } from 'react-markdown';
import remarkGfm from 'remark-gfm';

import { remarkNexus, TAG_SCHEME, WIKILINK_SCHEME } from '../../lib/markdown/remarkNexus';
import { tauriApi } from '../../lib/tauri-api';
import styles from './MarkdownPreview.module.css';

/**
 * Read-only рендер markdown (#20, по образцу Obsidian «Reading view»). CSP-безопасен: react-markdown
 * НЕ рендерит сырой HTML (rehype-raw не подключён) → без `dangerouslySetInnerHTML`/inline-обработчиков;
 * `urlTransform` режет `javascript:`/`data:`. GFM (таблицы/таск-листы/~~strike~~) + Nexus `[[wikilink]]`
 * (клик → навигация) и `#tag` (чип). Математика (KaTeX)/диаграммы (Mermaid) ОТЛОЖЕНЫ — требуют inline-
 * стилей, запрещённых строгим CSP (см. docs/dev/editor.md, BACKLOG). Live-preview (inline-правки) — пост-v1.
 */

/** Разрешает кастомные nexus-схемы и безопасные (http/https/mailto/tel/относительные); прочие → ''. */
function urlTransform(url: string): string {
  if (url.startsWith(WIKILINK_SCHEME) || url.startsWith(TAG_SCHEME)) return url;
  const hasScheme = /^[a-z][a-z0-9+.-]*:/i.test(url);
  return !hasScheme || /^(https?:|mailto:|tel:)/i.test(url) ? url : '';
}

export function MarkdownPreview({
  source,
  onOpenLink,
}: {
  source: string;
  onOpenLink: (target: string) => void;
}) {
  const components: Components = {
    a({ href, children }) {
      if (href && href.startsWith(WIKILINK_SCHEME)) {
        const target = decodeURIComponent(href.slice(WIKILINK_SCHEME.length));
        return (
          <a
            className={styles.wikilink}
            href="#"
            onClick={(e) => {
              e.preventDefault();
              onOpenLink(target);
            }}
          >
            {children}
          </a>
        );
      }
      if (href && href.startsWith(TAG_SCHEME)) {
        return <span className={styles.tag}>{children}</span>;
      }
      // Внешняя http(s)-ссылка: в Tauri-вебвью target=_blank мёртв → системный браузер через
      // opener. Прочие схемы (mailto/tel) и относительные — обычный `<a>` (их opener не берёт).
      const external = href && /^https?:\/\//i.test(href);
      return (
        <a
          href={href}
          target="_blank"
          rel="noreferrer noopener"
          onClick={
            external
              ? (e) => {
                  e.preventDefault();
                  void tauriApi.external.open(href).catch(() => {});
                }
              : undefined
          }
        >
          {children}
        </a>
      );
    },
  };

  return (
    <div className={styles.preview}>
      <ReactMarkdown
        remarkPlugins={[remarkGfm, remarkNexus]}
        urlTransform={urlTransform}
        components={components}
      >
        {source}
      </ReactMarkdown>
    </div>
  );
}
