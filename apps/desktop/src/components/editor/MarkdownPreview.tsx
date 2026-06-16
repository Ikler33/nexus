import { createElement, useContext, useEffect, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import ReactMarkdown from 'react-markdown';
import type { Components } from 'react-markdown';
import rehypeKatex from 'rehype-katex';
import remarkGfm from 'remark-gfm';
import remarkMath from 'remark-math';

import { isTaskLine } from '../../lib/editor/format';
import { EmbedContext } from '../../lib/markdown/embed-context';
import { rehypeKatexCsp } from '../../lib/markdown/rehypeKatexCsp';
import { remarkCallouts } from '../../lib/markdown/remarkCallouts';
import { remarkEmbeds } from '../../lib/markdown/remarkEmbeds';
import { remarkMermaid } from '../../lib/markdown/remarkMermaid';
import { remarkNexus, TAG_SCHEME, WIKILINK_SCHEME } from '../../lib/markdown/remarkNexus';
import { tauriApi } from '../../lib/tauri-api';
import { Callout, CalloutTitle } from './Callout';
import { MermaidDiagram } from './MermaidDiagram';
import { NoteEmbed } from './NoteEmbed';
import styles from './MarkdownPreview.module.css';

/**
 * Read-only рендер markdown (#20, по образцу Obsidian «Reading view»). CSP-безопасен: react-markdown
 * НЕ рендерит сырой HTML (rehype-raw не подключён) → без `dangerouslySetInnerHTML`/inline-обработчиков;
 * `urlTransform` режет `javascript:`/`data:`. GFM (таблицы/таск-листы/~~strike~~) + Nexus `[[wikilink]]`
 * (клик → навигация) и `#tag` (чип). Математика `$$…$$` (#4, инлайн и блок) — через remark-math +
 * rehype-katex с `output:'mathml'`: чистый нативный `<math>` БЕЗ inline-стилей и без шрифтов KaTeX → CSP
 * не трогаем (`rehypeKatexCsp` снимает инлайн-`style`, что KaTeX даёт на ошибках/`\fcolorbox`). Одиночный
 * `$` НЕ математика (`singleDollarTextMath:false`) — иначе суммы `$5…$10` в заметках о деньгах ломались бы.
 * Диаграммы (Mermaid) отложены. Live-preview (inline-правки) — пост-v1.
 */

/** Разрешает кастомные nexus-схемы и безопасные (http/https/mailto/tel/относительные); прочие → ''. */
function urlTransform(url: string): string {
  if (url.startsWith(WIKILINK_SCHEME) || url.startsWith(TAG_SCHEME)) return url;
  const hasScheme = /^[a-z][a-z0-9+.-]*:/i.test(url);
  // `data:image/…` разрешаем (inline-картинки, IMG-1 #213); НЕ весь `data:` — `data:text/html,<script>`
  // на href ссылки = XSS (urlTransform общий для href и src), поэтому только image-подтип (находка аудита).
  return !hasScheme || /^(https?:|mailto:|tel:|data:image\/)/i.test(url) ? url : '';
}

/** Минимальная форма hast-узла, по которой ищем состояние GFM-чекбокса (без зависимости от типов hast). */
type HastNode = { tagName?: string; properties?: Record<string, unknown>; children?: HastNode[] };

/** Состояние СОБСТВЕННОГО таск-чекбокса `li` из GFM-парса: первый `<input type=checkbox>` среди
 *  потомков, НЕ спускаясь во вложенные подсписки (`ul`/`ol`) — иначе отметка дочернего таска ложно
 *  подменила бы родительский. Tight-список держит input прямым ребёнком, loose — внутри `<p>`. */
function ownTaskChecked(node: HastNode | undefined): boolean {
  if (!node) return false;
  if (node.tagName === 'input' && node.properties?.type === 'checkbox') {
    return Boolean(node.properties.checked);
  }
  for (const child of node.children ?? []) {
    if (child.tagName === 'ul' || child.tagName === 'ol') continue; // не спускаемся в подсписок
    if (ownTaskChecked(child)) return true;
  }
  return false;
}

/**
 * Картинка-вложение в превью (IMG-1). Vault-относительный путь (`attachments/…`) грузится как
 * `data:`-URL через `read_attachment` (CSP разрешает `data:`, asset-протокол не нужен); внешние
 * `http(s):`/`data:` остаются как есть. `alt` прокидывается из markdown — без нарушений CSP.
 */
function VaultImage({ src, alt, width }: { src?: string; alt?: string; width?: number }) {
  const external = !src || /^(https?:|data:)/i.test(src);
  const [resolved, setResolved] = useState<string | undefined>(undefined);
  useEffect(() => {
    if (!src || external) return;
    let alive = true;
    void tauriApi.attachments
      .read(src)
      .then((url) => {
        if (alive && url) setResolved(url);
      })
      .catch(() => {});
    return () => {
      alive = false;
    };
  }, [src, external]);
  // `width` — HTML-атрибут (презентационный), НЕ inline-style → CSP `style-src` не нарушаем.
  return (
    <img
      className={styles.image}
      src={external ? src : resolved}
      alt={alt ?? ''}
      width={width}
      loading="lazy"
    />
  );
}

/**
 * Картинка-вставка `![[pic.png]]` / `![[pic.png|alt|300]]` (IMG-EMBED, Live-Preview). Резолвит basename
 * → относительный путь vault командой `resolve_attachment` (картинки НЕ в индексе — обход ФС), затем
 * рендерит через `VaultImage` (тот же `read_attachment` → `data:`-URL). Не найдено → честная заглушка.
 */
function EmbedImage({ name, alt, width }: { name: string; alt: string; width?: number }) {
  const { t } = useTranslation();
  const [state, setState] = useState<'loading' | 'ok' | 'missing'>('loading');
  const [path, setPath] = useState<string | undefined>(undefined);
  useEffect(() => {
    let alive = true;
    void tauriApi.attachments
      .resolve(name)
      .then((p) => {
        if (!alive) return;
        if (p) {
          setPath(p);
          setState('ok');
        } else {
          setState('missing');
        }
      })
      .catch(() => {
        if (alive) setState('missing');
      });
    return () => {
      alive = false;
    };
  }, [name]);
  if (state === 'missing') return <span className={styles.embedNote}>{t('embed.imageMissing', { name })}</span>;
  if (state === 'loading') return <span className={styles.embedNote}>{t('embed.loading')}</span>;
  return <VaultImage src={path} alt={alt || name} width={width} />;
}

export function MarkdownPreview({
  source,
  onOpenLink,
  onToggleTask,
  notePath,
}: {
  source: string;
  onOpenLink: (target: string) => void;
  /** EDIT-5: клик по чекбоксу таска в превью → 1-based номер исходной строки. Не задан — чекбоксы
   *  остаются read-only (дефолтный disabled-рендер GFM), как в любых не-редактируемых контекстах. */
  onToggleTask?: (line: number) => void;
  /** Путь ЭТОЙ заметки — заносится в предки гард-цикла транклюзии (`![[self]]` ловится). Не задан
   *  (доска/peek) — гард работает по глубине и по предкам вложенных вставок, без само-вставки корня. */
  notePath?: string;
}) {
  // Транклюзия: добавляем свой путь в множество предков (гард-цикл A→B→A). Мемо — стабильная
  // идентичность Set'а, иначе вложенный NoteEmbed перефетчивал бы на каждый ре-рендер родителя.
  const inheritedEmbed = useContext(EmbedContext);
  const embedCtx = useMemo(
    () =>
      notePath
        ? { ancestors: new Set([...inheritedEmbed.ancestors, notePath]), depth: inheritedEmbed.depth }
        : inheritedEmbed,
    [inheritedEmbed, notePath],
  );

  const sourceLines = onToggleTask ? source.split('\n') : null;
  const components: Components = {
    // IMG-1: картинки-вложения через VaultImage (vault-путь → data:-URL).
    img({ src, alt }) {
      return (
        <VaultImage
          src={typeof src === 'string' ? src : undefined}
          alt={typeof alt === 'string' ? alt : undefined}
        />
      );
    },
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

  // EDIT-7: помечаем заголовки исходной строкой (`data-outline-line`) — панель Outline скроллит к
  // ним в режиме чтения/превью (в source-режиме переход идёт через CM6). `node.position.start.line` —
  // тот же источник позиции, что у тасков (EDIT-5); атрибут невидимый, рендер заголовков не меняет.
  const headingWithLine =
    (tag: 'h1' | 'h2' | 'h3' | 'h4' | 'h5' | 'h6'): Components['h1'] =>
    ({ node, children }) => {
      const line = node?.position?.start?.line;
      return createElement(tag, typeof line === 'number' ? { 'data-outline-line': line } : {}, children);
    };
  components.h1 = headingWithLine('h1');
  components.h2 = headingWithLine('h2');
  components.h3 = headingWithLine('h3');
  components.h4 = headingWithLine('h4');
  components.h5 = headingWithLine('h5');
  components.h6 = headingWithLine('h6');

  // Транклюзия: кастомный блок `nexus-embed` (из remarkEmbeds) → рекурсивный рендер вставленной
  // заметки. Кастомный тег вне `Components`-типа → регистрируем индексом. Нагрузку (target/anchor)
  // берём из hast-properties (hProperties копируются в node.properties дословно).
  (components as Record<string, Components['div']>)['nexus-embed'] = ({ node }) => {
    const props = (node?.properties ?? {}) as Record<string, unknown>;
    const target = typeof props.target === 'string' ? props.target : '';
    const anchor = typeof props.anchor === 'string' ? props.anchor : '';
    if (!target) return null;
    return (
      <NoteEmbed
        target={target}
        anchor={anchor}
        onOpenLink={onOpenLink}
        renderBody={(section, np) => (
          <MarkdownPreview source={section} onOpenLink={onOpenLink} notePath={np} />
        )}
      />
    );
  };

  // Картинка-вставка `nexus-image` (из remarkEmbeds) → резолв basename + рендер `<img>`.
  (components as Record<string, Components['div']>)['nexus-image'] = ({ node }) => {
    const props = (node?.properties ?? {}) as Record<string, unknown>;
    const name = typeof props.name === 'string' ? props.name : '';
    const alt = typeof props.alt === 'string' ? props.alt : '';
    const widthStr = typeof props.width === 'string' ? props.width : '';
    if (!name) return null;
    const width = /^\d+$/.test(widthStr) ? Number(widthStr) : undefined;
    return <EmbedImage name={name} alt={alt} width={width} />;
  };

  // Mermaid-диаграмма `nexus-mermaid` (из remarkMermaid) → ленивый рендер CSP-безопасного SVG.
  (components as Record<string, Components['div']>)['nexus-mermaid'] = ({ node }) => {
    const code = (node?.properties as Record<string, unknown> | undefined)?.code;
    return typeof code === 'string' && code.trim() ? <MermaidDiagram code={code} /> : null;
  };

  // Callout `nexus-callout` (из remarkCallouts) → admonition-блок: иконка/цвет по типу, опц. сворачивание.
  (components as Record<string, Components['div']>)['nexus-callout'] = ({ node, children }) => {
    const props = (node?.properties ?? {}) as Record<string, unknown>;
    const kind = typeof props.kind === 'string' ? props.kind : 'note';
    const fold = typeof props.fold === 'string' ? props.fold : '';
    return (
      <Callout kind={kind} fold={fold}>
        {children}
      </Callout>
    );
  };
  // Заголовок callout `nexus-callout-title` (иконка + подпись; пустой → дефолтная подпись по типу).
  (components as Record<string, Components['div']>)['nexus-callout-title'] = ({ node, children }) => {
    const props = (node?.properties ?? {}) as Record<string, unknown>;
    const kind = typeof props.kind === 'string' ? props.kind : 'note';
    const label = typeof props.label === 'string' ? props.label : kind;
    return (
      <CalloutTitle kind={kind} label={label}>
        {children}
      </CalloutTitle>
    );
  };

  if (onToggleTask) {
    // EDIT-5: убираем дефолтный disabled-чекбокс GFM (единственный источник `<input>` в markdown,
    // в т.ч. вложенный в `<p>` у loose-списков) — единственный чекбокс рисуем в `li`.
    components.input = () => null;
    components.li = ({ node, className, children }) => {
      const cls = String(className ?? '');
      if (!cls.includes('task-list-item')) return <li className={cls || undefined}>{children}</li>;
      // Состояние — авторитетное из GFM-парса (а не из перепарса исходной строки): корректно для
      // цитат/вложенности. Интерактив — только если исходная строка реально тогглится (toggleTaskAtLine);
      // иначе (таск в цитате `> - [ ]`, узел без позиции) — честный read-only disabled, не мёртвый клик.
      const line = node?.position?.start?.line;
      const togglable = typeof line === 'number' && isTaskLine(sourceLines?.[line - 1] ?? '');
      return (
        <li className={cls}>
          <input
            type="checkbox"
            className={styles.taskCheckbox}
            checked={ownTaskChecked(node as HastNode | undefined)}
            disabled={!togglable}
            readOnly={!togglable}
            onChange={togglable ? () => onToggleTask(line) : undefined}
          />
          {children}
        </li>
      );
    };
  }

  return (
    <EmbedContext.Provider value={embedCtx}>
      <div className={styles.preview}>
        <ReactMarkdown
          remarkPlugins={[remarkEmbeds, remarkMermaid, remarkCallouts, remarkGfm, remarkNexus, [remarkMath, { singleDollarTextMath: false }]]}
          rehypePlugins={[[rehypeKatex, { output: 'mathml', throwOnError: false, strict: false }], rehypeKatexCsp]}
          urlTransform={urlTransform}
          components={components}
        >
          {source}
        </ReactMarkdown>
      </div>
    </EmbedContext.Provider>
  );
}
