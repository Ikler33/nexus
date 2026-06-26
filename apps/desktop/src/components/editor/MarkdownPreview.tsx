import type { ComponentProps, ForwardedRef, MouseEvent as ReactMouseEvent } from 'react';
import { createElement, forwardRef, useCallback, useContext, useEffect, useImperativeHandle, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { Clock } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import ReactMarkdown from 'react-markdown';
import type { Components } from 'react-markdown';
import rehypeKatex from 'rehype-katex';
import remarkGfm from 'remark-gfm';
import remarkMath from 'remark-math';

import { isTaskLine } from '../../lib/editor/format';
import { deriveMasthead, dropCapLetter } from '../../lib/editor/masthead';
import {
  bodyExcerpt,
  footnoteNumber,
  footnoteText,
  noteStatus,
  noteTitle,
  noteType,
} from '../../lib/editor/popcard';
import { makeSlugger } from '../../lib/editor/slug';
import { EmbedContext } from '../../lib/markdown/embed-context';
import { extractFrontmatter, parseFrontmatterFields } from '../../lib/markdown/frontmatter';
import { rehypeKatexCsp } from '../../lib/markdown/rehypeKatexCsp';
import { rehypeSections } from '../../lib/markdown/rehypeSections';
import { SectionContext } from '../../lib/markdown/section-context';
import { remarkCallouts } from '../../lib/markdown/remarkCallouts';
import { remarkComments } from '../../lib/markdown/remarkComments';
import { remarkEmbeds } from '../../lib/markdown/remarkEmbeds';
import { remarkFrontmatter } from '../../lib/markdown/remarkFrontmatter';
import { remarkHighlight } from '../../lib/markdown/remarkHighlight';
import { remarkMermaid } from '../../lib/markdown/remarkMermaid';
import { remarkNexus, TAG_SCHEME, WIKILINK_SCHEME } from '../../lib/markdown/remarkNexus';
import { remarkStripHeadingEmoji } from '../../lib/markdown/remarkStripHeadingEmoji';
import { tauriApi, type NoteRef } from '../../lib/tauri-api';
import { relTime } from '../../lib/time';
import { AppendLine } from './AppendLine';
import { Callout, CalloutTitle } from './Callout';
import { MermaidDiagram } from './MermaidDiagram';
import { NoteEmbed } from './NoteEmbed';
import { Popcard, type PopcardContent } from './Popcard';
import { PropertiesTable } from './PropertiesTable';
import { Section } from './Section';
import { SectionHeading } from './SectionHeading';
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

// remark/rehype-цепочки — ЧИСТЫЕ КОНСТАНТЫ (плагины и их опции не зависят от пропсов/стейта). Вынесены
// в модуль-скоуп: иначе свежие литералы-массивы на каждый ре-рендер ломали бы мемоизацию `<ReactMarkdown>`
// (S6-FIX1: смена activeLine в родителе НЕ должна гонять весь remark/rehype-пайплайн заново).
// ПОРЯДОК ВАЖЕН: remarkFrontmatter/remarkComments — ПЕРВЫМИ (убрать frontmatter и вырезать `%%…%%` до
// embed/callout/nexus). pass-2 remarkComments чистит пустые абзацы, а remarkEmbeds позже делает embed-узлы
// «пустыми» абзацами → reorder remarkComments ПОСЛЕ embeds стёр бы вставки.
const REMARK_PLUGINS: ComponentProps<typeof ReactMarkdown>['remarkPlugins'] = [
  remarkFrontmatter,
  remarkComments,
  remarkEmbeds,
  remarkMermaid,
  remarkCallouts,
  remarkGfm,
  remarkHighlight,
  // Эмодзи из заголовков (Hermes-8 фикс) — ДО remarkNexus: тот сплитит [[вики]]/#теги в заголовке в
  // link-узлы, а strip бьёт text-узлы; режем эмодзи пока заголовок ещё цельный text → вики/теги целы.
  remarkStripHeadingEmoji,
  remarkNexus,
  [remarkMath, { singleDollarTextMath: false }],
];
// rehypeSections — ПОСЛЕ katex/csp: ему нужен полный, стабильный hast root (он лишь ПЕРЕГРУППИРОВЫВАЕТ узлы
// в `<section.sec>`, не трогая содержимое). h2 перемещаются as-is → React-оверрайд проставит id/data-outline-line.
const REHYPE_PLUGINS: ComponentProps<typeof ReactMarkdown>['rehypePlugins'] = [
  [rehypeKatex, { output: 'mathml', throwOnError: false, strict: false }],
  rehypeKatexCsp,
  rehypeSections,
];

/** Минимальная форма hast-узла, по которой ищем состояние GFM-чекбокса (без зависимости от типов hast). */
type HastNode = { tagName?: string; properties?: Record<string, unknown>; children?: HastNode[] };

/** Плоский текст hast-узла (для slug заголовка, HEADANCHOR-1): рекурсивно собираем `value` text-узлов. */
function hastText(node: { value?: string; children?: unknown[] }): string {
  if (typeof node.value === 'string') return node.value;
  if (Array.isArray(node.children)) return node.children.map((c) => hastText(c as { value?: string })).join('');
  return '';
}

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

/**
 * Hermes-8 S6: imperative-хэндл превью для GroupPane. `revealLine(line)` раскрывает свёрнутую S3-секцию,
 * содержащую исходную строку `line` (чтобы прыжок оглавления к скрытому h3 доскроллил к видимой цели).
 */
export interface MarkdownPreviewHandle {
  /** Раскрывает свёрнутую S3-секцию, содержащую строку `line`. Возвращает `true`, если реально раскрыл
   *  (вызвал grid-анимацию) — GroupPane по этому признаку откладывает scrollIntoView до конца раскрытия. */
  revealLine: (line: number) => boolean;
}

function MarkdownPreviewImpl(
  {
    source,
    onOpenLink,
    onOpenTag,
    onToggleTask,
    notePath,
    onAppendLine,
    fetchNotes,
    masthead,
  }: {
    source: string;
  onOpenLink: (target: string) => void;
  /** TAGCLICK-1: клик по `#tag`-чипу → имя тега (без `#`). Не задан (доска/peek/вложенный embed без
   *  проброса) — чип остаётся НЕ-кликабельным `<span>` (честно, как onToggleTask-absence у чекбоксов). */
  onOpenTag?: (tag: string) => void;
  /** EDIT-5: клик по чекбоксу таска в превью → 1-based номер исходной строки. Не задан — чекбоксы
   *  остаются read-only (дефолтный disabled-рендер GFM), как в любых не-редактируемых контекстах. */
  onToggleTask?: (line: number) => void;
  /** Путь ЭТОЙ заметки — заносится в предки гард-цикла транклюзии (`![[self]]` ловится). Не задан
   *  (доска/peek) — гард работает по глубине и по предкам вложенных вставок, без само-вставки корня. */
  notePath?: string;
  /** AppendLine (макет): дописать строку в конец заметки через буфер. Задан ТОЛЬКО для top-level
   *  превью редактора (GroupPane) — у вложенных embed/peek/доски не задан → quick-add не рисуется. */
  onAppendLine?: (line: string) => void;
  /** Заметки по подстроке для `[[…` автокомплита AppendLine (тот же источник, что у CM6). */
  fetchNotes?: (query: string) => Promise<NoteRef[]>;
  /** MASTHEAD-1 (Hermes-6 editor.jsx): editorial-шапка (kicker/title/byline) + буквица ведущего абзаца.
   *  Задаётся ТОЛЬКО для top-level превью редактора (GroupPane, режим чтения/просмотра). Не задан
   *  (embed/peek/доска) — шапки и буквицы нет (как у вложенных рендеров макета). `mtime` — для chip'а
   *  «изменено» (живёт в GroupPane); `reading` — режим чтения ⌘R (центрированная шапка, крупнее буквица). */
  masthead?: { mtime: number | null; reading?: boolean };
  },
  ref: ForwardedRef<MarkdownPreviewHandle>,
) {
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

  const { t, i18n } = useTranslation();

  // S3 «Редакция»: свёрнутые H2-секции — Set по `data-sec-id` (slug заголовка, стабилен к правкам в
  // других секциях). По умолчанию все РАЗВЁРНУТЫ (пустой Set). Тоггл — на клик/Enter/Space по h2.
  // Состояние читают РАЗНЫЕ оверрайды (`h2`-кнопка, `section`-обёртка) → раздаём через SectionContext.
  const [collapsedSecs, setCollapsedSecs] = useState<Set<string>>(() => new Set());
  const toggleSection = useCallback((id: string) => {
    setCollapsedSecs((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  }, []);
  const sectionState = useMemo(
    () => ({ isCollapsed: (id: string) => collapsedSecs.has(id), toggle: toggleSection }),
    [collapsedSecs, toggleSection],
  );
  // Сброс свёрнутости при смене ЗАМЕТКИ (`notePath`): иначе stale secid'ы прежней заметки копятся в Set
  // (рост памяти) и одноимённая секция новой заметки открывалась бы уже свёрнутой. На правку source ТОЙ ЖЕ
  // заметки НЕ сбрасываем — иначе свёрнутость терялась бы на каждое нажатие клавиши (нежелательно).
  useEffect(() => {
    setCollapsedSecs((prev) => (prev.size === 0 ? prev : new Set()));
  }, [notePath]);

  // MASTHEAD-1: данные editorial-шапки. Считаем всегда (дёшево, мемо), используем только когда задан
  // `masthead` (top-level превью). `body` — исходник с ОБНУЛЁННЫМ ведущим H1 (его текст ушёл в заголовок
  // шапки): обнуление, а не удаление, сохраняет номера строк для тоггла тасков/оглавления (см. masthead.ts).
  const md = useMemo(() => deriveMasthead(source, notePath), [source, notePath]);
  const mastheadActive = masthead != null;
  const body = mastheadActive ? md.body : source;
  const words = useMemo(() => source.split(/\s+/).filter(Boolean).length, [source]);
  const readingMinutes = Math.max(1, Math.round(words / 200));

  // Буквица ведущего абзаца (порт dropcap.js): после коммита находим первый блок тела (первый ребёнок
  // `.preview` ПОСЛЕ шапки и Properties-таблицы) и, если это абзац, штампуем его первую букву в `data-cap`
  // (CSS тюнит оптический зазор по глифу) + маркер `data-dropcap`. Только в режиме шапки; иначе снимаем.
  const previewRef = useRef<HTMLDivElement>(null);

  // Hermes-8 S6: imperative-хэндл для GroupPane — раскрытие свёрнутой секции при прыжке оглавления.
  // ИСТОЧНИК = DOM (надёжнее перепарса структуры): по `[data-outline-line="${line}"]` находим узел
  // целевой строки в ВЛАДЕЮЩЕЙ .preview (previewRef, не спускаясь в чужие эмбеды), поднимаемся до его
  // секции `<section.sec data-sec-id>` и убираем secId из `collapsedSecs`. Идемпотентно: секции нет
  // (строка вне секций — лид/footnotes) или она уже развёрнута → no-op (Set без secId не меняется).
  // Прыжок к самому h2 (его строка) тоже раскрывает тело — ок по UX (показать содержимое при переходе
  // к заголовку секции). React.lazy + forwardRef совместимы: ref доходит через lazy-границу (тест S6).
  useImperativeHandle(
    ref,
    () => ({
      // Возвращает `true`, если РЕАЛЬНО раскрыл свёрнутую секцию (GroupPane по этому признаку откладывает
      // scrollIntoView до конца grid-анимации раскрытия — S6-FIX2). `false` = секции нет / уже развёрнута →
      // прыжок можно скроллить немедленно. Состояние свёрнутости читаем ИЗ DOM (класс `.collapsed` на секции,
      // тот же `styles.collapsed`, что ставит Section) — синхронный источник истины, без stale-замыкания на
      // collapsedSecs (хэндл мемоизирован с []).
      revealLine(line: number): boolean {
        const root = previewRef.current;
        if (!root) return false;
        // `line` — доверенное целое (из outline.extractHeadings) → CSS.escape не нужен (он дал бы
        // путаную digit-escape-форму `\33 ` в строке селектора). Нечисловое отсекаем заранее.
        if (!Number.isFinite(line)) return false;
        const target = root.querySelector<HTMLElement>(`[data-outline-line="${line}"]`);
        // Фильтр по owning-preview: querySelector мог бы взять одноимённую строку вложенного эмбеда.
        if (!target || target.closest('[class*="preview"]') !== root) return false;
        const sec = target.closest<HTMLElement>('section[data-sec-id]');
        const secId = sec?.getAttribute('data-sec-id');
        if (!sec || !secId) return false;
        const wasCollapsed = sec.classList.contains(styles.collapsed);
        if (!wasCollapsed) return false; // уже развёрнута → no-op, скролл немедленный
        setCollapsedSecs((prev) => {
          if (!prev.has(secId)) return prev; // защита от гонки (стабильная ссылка — без лишнего ре-рендера)
          const next = new Set(prev);
          next.delete(secId);
          return next;
        });
        return true; // раскрыли свёрнутую секцию → GroupPane ждёт анимацию перед скроллом
      },
    }),
    [],
  );

  useLayoutEffect(() => {
    const root = previewRef.current;
    if (!root) return;
    root.querySelectorAll('[data-dropcap]').forEach((el) => {
      el.removeAttribute('data-dropcap');
      el.removeAttribute('data-cap');
    });
    if (!mastheadActive) return;
    let el: Element | null = root.firstElementChild;
    while (el && (el.classList.contains(styles.docHead) || el.classList.contains(styles.properties))) {
      el = el.nextElementSibling;
    }
    // Цель буквицы — ведущий абзац тела. Прямой `<P>` (заметка открывается прозой) штампуем как есть.
    // Если же первый блок — H2-секция (`<section.sec>`, S3: заметка открывается заголовком, напр. daily
    // `## 🧠 Поток мыслей`), её первый абзац вложен в `<section><div.sec-body><div.sec-inner><p>` →
    // спускаемся в ПЕРВУЮ контент-секцию и берём первый `<p>` в порядке чтения. adversarial FIX 3:
    // ПРОПУСКАЕМ абзацы внутри callout (`<div data-callout>`, см. Callout.tsx) и `<blockquote>` — иначе
    // гигантская буквица села бы внутрь admonition/цитаты раньше реального лид-абзаца секции. Нет
    // подходящего «голого» `<p>` → буквицы нет (как list-first).
    let target: HTMLElement | null = null;
    if (el && el.tagName === 'P') {
      target = el as HTMLElement;
    } else if (el && (el.tagName === 'SECTION' || el.classList.contains(styles.sec))) {
      target =
        Array.from(el.querySelectorAll('p')).find((p) => !p.closest('[data-callout],blockquote')) ?? null;
    }
    if (target) {
      const cap = dropCapLetter(target.textContent ?? '');
      if (cap) {
        target.setAttribute('data-cap', cap);
        target.setAttribute('data-dropcap', '');
      }
    }
    // deps — примитивы (mastheadActive), а НЕ объект `masthead`: иначе свежий литерал {mtime,reading}
    // на каждый ре-рендер GroupPane перезапускал бы эффект вхолостую. Штамповка зависит только от body.
  }, [body, mastheadActive]);

  // ── Hermes-8 S7: ховер-превью `.popcard` (вики-ссылка 220мс / сноска 120мс) ──────────────────────
  // Карточка одна; `rect` — точка привязки (от `getBoundingClientRect` триггера). `pendingRef` держит
  // активный таймер показа; `tokenRef` — монотонный request-токен против гонок async-чтения заметки
  // (ушли с ссылки/сменили цель за время чтения → старый ответ НЕ показывается).
  const [popcard, setPopcard] = useState<{
    variant: 'wiki' | 'fnote';
    rect: { top: number; bottom: number; left: number };
    content: PopcardContent;
  } | null>(null);
  const pendingRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const tokenRef = useRef(0);

  // Гасит pending-таймер и инвалидирует текущий request-токен (любой in-flight ответ устаревает).
  const cancelPending = useCallback(() => {
    if (pendingRef.current != null) {
      clearTimeout(pendingRef.current);
      pendingRef.current = null;
    }
    tokenRef.current += 1;
  }, []);

  // Полное скрытие: гасим таймер/токен и убираем карточку (на mouseleave триггера).
  const hidePopcard = useCallback(() => {
    cancelPending();
    setPopcard(null);
  }, [cancelPending]);

  // Очистка таймеров/токена на unmount (без утечек).
  useEffect(() => {
    return () => {
      cancelPending();
    };
  }, [cancelPending]);
  // Превью перерисовалось (СМЕНА заметки ИЛИ живое редактирование того же notePath: `body` поменялся) →
  // гасим pending-таймер и прячем stale-карточку: её rect привязан к прежнему DOM, цель/триггер могли
  // уехать (ревью MINOR: при правке тела карточка переживала ре-рендер со stale-rect). `body` меняется
  // и при смене notePath (другой source), поэтому покрывает оба случая.
  useEffect(() => {
    cancelPending();
    setPopcard(null);
  }, [body, cancelPending]);

  // Наведение на ВИКИ-ссылку: 220мс → резолв имени в заметку (resolveNote→readFile), РЕАЛЬНОЕ превью.
  const onWikiEnter = useCallback(
    (e: ReactMouseEvent<HTMLAnchorElement>, target: string) => {
      cancelPending();
      const rect = e.currentTarget.getBoundingClientRect();
      const place = { top: rect.top, bottom: rect.bottom, left: rect.left };
      const myToken = tokenRef.current;
      pendingRef.current = setTimeout(() => {
        void (async () => {
          try {
            // 1) имя → путь (бэкенд-резолвер индексатора — путь/+.md/basename/алиас).
            const path = await tauriApi.vault.resolveNote(target);
            if (tokenRef.current !== myToken) return; // ушли/сменили за время резолва
            if (!path) {
              // Битая ссылка → ЧЕСТНОЕ «не найдено» (muted), не фейк-превью.
              setPopcard({ variant: 'wiki', rect: place, content: { body: t('popcard.notFound'), muted: true } });
              return;
            }
            // 2) путь → содержимое (реальный текст заметки).
            const content = await tauriApi.vault.readFile(path);
            if (tokenRef.current !== myToken) return;
            // 3) реальные беклинки целевой заметки → счётчик ссылок (НЕ выдуманный).
            let linkCount: number | null = null;
            try {
              const backlinks = await tauriApi.graph.getBacklinks(path);
              if (tokenRef.current !== myToken) return;
              linkCount = backlinks.length;
            } catch {
              linkCount = null; // источник недоступен → счётчик опускаем (не врём)
            }
            const type = noteType(content);
            const status = noteStatus(content);
            // `.pc-meta` — ТОЛЬКО реальные данные: статус (frontmatter) и/или N ссылок (реальные беклинки).
            const metaParts: string[] = [];
            if (status) metaParts.push(status);
            if (linkCount != null && linkCount > 0) metaParts.push(t('popcard.links', { count: linkCount }));
            const body = bodyExcerpt(content);
            // Заметка НАЙДЕНА, но эксцерпт пуст (только frontmatter/H1) — честное «Пустая заметка»
            // (muted), но title/meta — реальные, СОХРАНЯЕМ (ревью: «не найдена» для существующей заметки
            // — ложь; `popcard.notFound` остаётся ТОЛЬКО в ветке path===null выше).
            setPopcard({
              variant: 'wiki',
              rect: place,
              content: {
                eyebrow: type ? type.toUpperCase() : undefined,
                title: noteTitle(content, path),
                body: body || t('popcard.empty'),
                muted: body ? undefined : true,
                meta: metaParts.length > 0 ? metaParts.join(' · ') : undefined,
              },
            });
          } catch {
            // Транзиентная ошибка чтения — карточку не показываем (молча), как другие read-failures превью.
          }
        })();
      }, 220);
    },
    [cancelPending, t],
  );

  // Наведение на СНОСКУ (footnote-ref `<a href="#user-content-fn-N">`): 120мс → текст из её `<li>` в DOM.
  const onFootnoteEnter = useCallback(
    (e: ReactMouseEvent<HTMLAnchorElement>, href: string) => {
      cancelPending();
      const rect = e.currentTarget.getBoundingClientRect();
      const place = { top: rect.top, bottom: rect.bottom, left: rect.left };
      const myToken = tokenRef.current;
      pendingRef.current = setTimeout(() => {
        if (tokenRef.current !== myToken) return;
        const n = footnoteNumber(href);
        if (!n) return;
        // Определение сноски ищем СТРОГО в ВЛАДЕЮЩЕЙ .preview этого инстанса (previewRef, стабилен).
        // `querySelector` спускается во вложенные .preview эмбедов (у них тот же `fn-1`, секция раньше
        // в DOM) → фильтруем по тому, чей ближайший .preview-предок === наш root: чужая сноска эмбеда
        // отбрасывается (MAJOR-ревью: показывалась сноска ИЗ эмбеда).
        const root = previewRef.current;
        if (!root) return;
        const li =
          Array.from(root.querySelectorAll(`li[id$="fn-${CSS.escape(n)}"]`)).find(
            (x) => x.closest('[class*="preview"]') === root,
          ) ?? null;
        const text = footnoteText(li);
        if (!text) return; // нет текста — карточку не показываем (честно)
        setPopcard({
          variant: 'fnote',
          rect: place,
          content: { eyebrow: t('popcard.footnote'), body: text },
        });
      }, 120);
    },
    [cancelPending, t],
  );

  // S6-FIX1: набор component-оверрайдов МЕМОИЗИРУЕМ — иначе свежий объект на каждый ре-рендер (в т.ч.
  // паразитный от смены activeLine в родителе) форсил бы ReactMarkdown заново прогонять весь remark/rehype-
  // пайплайн. Deps — ВСЁ, что реально замыкают оверрайды: пропсы-коллбэки (onOpenLink/onOpenTag/onToggleTask;
  // onAppendLine не тут — он в JSX, не в оверрайдах), стабильные useCallback-хэндлеры ховера (onWikiEnter/
  // onFootnoteEnter/hidePopcard) и производные от source/masthead (sourceLines/slugger/leadSlug). ВАЖНО:
  // оверрайды section/h2 НЕ замыкают collapsedSecs — они читают его через SectionContext, поэтому смена
  // свёрнутости НЕ инвалидирует `components` (S3 жив через контекст, см. ниже SectionContext.Provider).
  // Мемо отдаёт И `leadSlug`: slug ведущего H1 потребляется ТЕМ ЖЕ slugger'ом, что нумерует заголовки тела
  // (общий дедуп-счётчик), и нужен снаружи для `id` masthead-h1 — поэтому считаем здесь, отдаём наружу.
  const { components, leadSlug } = useMemo<{ components: Components; leadSlug: string | null }>(() => {
    const sourceLines = onToggleTask ? source.split('\n') : null;
    const comps: Components = {
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
        // S7: `data-note` + ховер-превью (220мс). Клик → живая навигация `onOpenLink` как раньше
        // (карточка `pointer-events:none` не перехватывает клик). Скрытие — на mouseleave.
        return (
          <a
            className={styles.wikilink}
            href="#"
            data-note={target}
            onMouseEnter={(e) => onWikiEnter(e, target)}
            onMouseLeave={hidePopcard}
            onClick={(e) => {
              e.preventDefault();
              hidePopcard();
              onOpenLink(target);
            }}
          >
            {children}
          </a>
        );
      }
      if (href && href.startsWith(TAG_SCHEME)) {
        // TAGCLICK-1: кликабельный чип (фильтр сайдбара по тегу), если задан onOpenTag. Иначе —
        // не-кликабельный `<span>` (embed/peek-контексты): честно, без мёртвого клика. `<span
        // role=button>` (а не `<a>`) — чтобы `.preview a` не перебивал стиль .tag своей специфичностью.
        // `.toLowerCase()` ОБЯЗАТЕЛЕН: бэкенд хранит теги в нижнем регистре (parser `tag.to_lowercase()`),
        // а `notes_by_tag` — точный матч; без нормализации `#TODO` дал бы пустую выдачу (ревью MAJOR).
        const tag = decodeURIComponent(href.slice(TAG_SCHEME.length)).toLowerCase();
        if (!onOpenTag) return <span className={styles.tag}>{children}</span>;
        return (
          <span
            className={styles.tag}
            role="button"
            tabIndex={0}
            onClick={() => onOpenTag(tag)}
            onKeyDown={(e) => {
              if (e.key === 'Enter' || e.key === ' ') {
                e.preventDefault();
                onOpenTag(tag);
              }
            }}
          >
            {children}
          </span>
        );
      }
      // FOOTNOTE-1/HEADANCHOR-1: внутренний якорь `#id` (back-ref сносок GFM, заголовки) → плавный
      // скролл В ПРЕДЕЛАХ этого превью. Не `target=_blank` (он ломал бы хеш-навигацию); область —
      // ближайший `.preview`, чтобы сноски двух embed'ов с одинаковым `#fn-1` не прыгали в чужой блок.
      if (href && href.startsWith('#') && href.length > 1) {
        // S7: footnote-ref (`#user-content-fn-N`) — ховер-превью текста сноски (120мс). Клик
        // (скролл к определению) и backref-стрелка — без изменений. Не-сноска `#anchor` — без ховера.
        const isFootnoteRef = /^#user-content-fn-/.test(href);
        return (
          <a
            href={href}
            onMouseEnter={isFootnoteRef ? (e) => onFootnoteEnter(e, href) : undefined}
            onMouseLeave={isFootnoteRef ? hidePopcard : undefined}
            onClick={(e) => {
              e.preventDefault();
              if (isFootnoteRef) hidePopcard();
              // RAW user-href: `decodeURIComponent` бросает URIError на литеральном `%` (`#50%`) →
              // гард, иначе клик молча падает (находка ревью). CSS.escape — против селектор-инъекции.
              let id = href.slice(1);
              try {
                id = decodeURIComponent(id);
              } catch {
                /* кривое %-кодирование — берём как есть */
              }
              // Скролл СТРОГО в ВЛАДЕЮЩЕЙ .preview этого инстанса (previewRef): `querySelector` спустился
              // бы во вложенные .preview эмбедов (тот же `user-content-fn-1`/slug) → фильтр по owning-preview,
              // иначе клик прыгал бы в чужой эмбед-блок (тот же класс дефекта, что hover — MAJOR-ревью).
              const root = previewRef.current;
              const el = root
                ? Array.from(root.querySelectorAll(`[id="${CSS.escape(id)}"]`)).find(
                    (x) => x.closest('[class*="preview"]') === root,
                  )
                : document.querySelector(`[id="${CSS.escape(id)}"]`);
              if (el instanceof HTMLElement) el.scrollIntoView({ behavior: 'smooth', block: 'center' });
            }}
          >
            {children}
          </a>
        );
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

  // HEADANCHOR-1: slug-id на заголовок (дедуп per memo-build) — якорь для `#heading`-навигации/сносок.
  // Новый slugger при каждой пересборке `components` (deps source/masthead/md), т.е. при каждой реальной
  // ре-рендере ReactMarkdown → без утечки счётчиков между нотами; стабильный пока текст/режим не сменился.
  const slugger = makeSlugger();
  // HEADANCHOR-1: ведущий H1 погашен в теле → его slug-id переносим на заголовок шапки, ПОТРЕБЛЯЯ slug
  // первым (до рендера тела) — чтобы якорь `#slug-ведущего-H1` вёл к шапке И дедуп последующих одноимённых
  // заголовков не сдвинулся. slugify сам срежет inline-разметку, поэтому передаём сырой h1Text.
  const leadSlug = mastheadActive && md.h1Text != null ? slugger(md.h1Text) : null;
  // EDIT-7: помечаем заголовки исходной строкой (`data-outline-line`) — панель Outline скроллит к
  // ним в режиме чтения/превью (в source-режиме переход идёт через CM6). `node.position.start.line` —
  // тот же источник позиции, что у тасков (EDIT-5); атрибут невидимый, рендер заголовков не меняет.
  const headingWithLine =
    (tag: 'h1' | 'h2' | 'h3' | 'h4' | 'h5' | 'h6'): Components['h1'] =>
    ({ node, children }) => {
      const line = node?.position?.start?.line;
      const id = node ? slugger(hastText(node)) : undefined; // HEADANCHOR-1 (slug-якорь, дедуп с leadSlug)
      // S3: h2 c `data-sec-id` (его проставил rehypeSections) — заголовок секции: рендерим интерактивный
      // `SectionHeading` (шеврон + клик-сворачивание + a11y). `id`/`data-outline-line` он ставит на host-h2
      // как есть (HEADANCHOR-1 цел). secId — ключ из rehype-дедупа (отдельно от slug-якоря: под masthead
      // они могут различаться, и это намеренно — slug ведёт якорь, secId держит состояние сворачивания).
      const secId = (node?.properties as Record<string, unknown> | undefined)?.['data-sec-id'];
      if (tag === 'h2' && typeof secId === 'string' && secId) {
        return (
          <SectionHeading secId={secId} id={id ?? ''} outlineLine={typeof line === 'number' ? line : undefined}>
            {children}
          </SectionHeading>
        );
      }
      const props: Record<string, unknown> = {};
      if (typeof line === 'number') props['data-outline-line'] = line;
      if (id != null) props.id = id;
      return createElement(tag, props, children);
    };
  comps.h1 = headingWithLine('h1');
  comps.h2 = headingWithLine('h2');
  comps.h3 = headingWithLine('h3');
  comps.h4 = headingWithLine('h4');
  comps.h5 = headingWithLine('h5');
  comps.h6 = headingWithLine('h6');

  // Транклюзия: кастомный блок `nexus-embed` (из remarkEmbeds) → рекурсивный рендер вставленной
  // заметки. Кастомный тег вне `Components`-типа → регистрируем индексом. Нагрузку (target/anchor)
  // берём из hast-properties (hProperties копируются в node.properties дословно).
  (comps as Record<string, Components['div']>)['nexus-embed'] = ({ node }) => {
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
          <MarkdownPreview source={section} onOpenLink={onOpenLink} onOpenTag={onOpenTag} notePath={np} />
        )}
      />
    );
  };

  // Картинка-вставка `nexus-image` (из remarkEmbeds) → резолв basename + рендер `<img>`.
  (comps as Record<string, Components['div']>)['nexus-image'] = ({ node }) => {
    const props = (node?.properties ?? {}) as Record<string, unknown>;
    const name = typeof props.name === 'string' ? props.name : '';
    const alt = typeof props.alt === 'string' ? props.alt : '';
    const widthStr = typeof props.width === 'string' ? props.width : '';
    if (!name) return null;
    const width = /^\d+$/.test(widthStr) ? Number(widthStr) : undefined;
    return <EmbedImage name={name} alt={alt} width={width} />;
  };

  // Mermaid-диаграмма `nexus-mermaid` (из remarkMermaid) → ленивый рендер CSP-безопасного SVG.
  (comps as Record<string, Components['div']>)['nexus-mermaid'] = ({ node }) => {
    const code = (node?.properties as Record<string, unknown> | undefined)?.code;
    return typeof code === 'string' && code.trim() ? <MermaidDiagram code={code} /> : null;
  };

  // Callout `nexus-callout` (из remarkCallouts) → admonition-блок: иконка/цвет по типу, опц. сворачивание.
  (comps as Record<string, Components['div']>)['nexus-callout'] = ({ node, children }) => {
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
  (comps as Record<string, Components['div']>)['nexus-callout-title'] = ({ node, children }) => {
    const props = (node?.properties ?? {}) as Record<string, unknown>;
    const kind = typeof props.kind === 'string' ? props.kind : 'note';
    const label = typeof props.label === 'string' ? props.label : kind;
    return (
      <CalloutTitle kind={kind} label={label}>
        {children}
      </CalloutTitle>
    );
  };

  // S5 «Редакция»: `<hr>` (thematic break) → астеризм из трёх точек (средняя ember). Семантику разрыва
  // сохраняем через `role=separator` (как нативный hr); сами точки — пустые `<span>` без текста (чистый
  // декор, AT ничего не зачитывает). CSS `.asterism` (MarkdownPreview.module.css). markdown `---`/`***`/
  // `___` дают этот узел; frontmatter-`---` уже срезан remarkFrontmatter (спурьёзного hr нет — FRONTMATTER-1).
  comps.hr = () => (
    <div className={styles.asterism} role="separator">
      <span />
      <span />
      <span />
    </div>
  );

  // S3 «Редакция»: обёртка H2-секции (rehypeSections обернул h2+тело в `<section.sec data-sec-id>`).
  // Класс `.collapsed` ставит `Section` по контексту; интерактив (клик/шеврон) — на самом h2
  // (`SectionHeading`). h2 уже отрендерен через `comps.h2` (HEADANCHOR-1: id/data-outline-line целы).
  comps.section = ({ node, children, ...rest }) => {
    const props = (node?.properties ?? {}) as Record<string, unknown>;
    // react-markdown@10 кладёт `data-sec-id` в node.properties БЕЗ camelCase (литеральный ключ).
    const secId = typeof props['data-sec-id'] === 'string' ? props['data-sec-id'] : '';
    // Не наша секция (напр. GFM-блок сносок `<section class="footnotes" data-footnotes>`): рендерим
    // КАК ЕСТЬ с исходными props (react-markdown прокидывает className/data-* в `rest`) — иначе потеряли
    // бы `.footnotes`-класс (FOOTNOTE-1: стили/якоря сносок завязаны на него).
    if (!secId) return <section {...rest}>{children}</section>;
    return <Section secId={secId}>{children}</Section>;
  };

  if (onToggleTask) {
    // EDIT-5: убираем дефолтный disabled-чекбокс GFM (единственный источник `<input>` в markdown,
    // в т.ч. вложенный в `<p>` у loose-списков) — единственный чекбокс рисуем в `li`.
    comps.input = () => null;
    comps.li = ({ node, className, children }) => {
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
    return { components: comps, leadSlug };
  }, [
    onOpenLink,
    onOpenTag,
    onToggleTask,
    onWikiEnter,
    onFootnoteEnter,
    hidePopcard,
    source,
    mastheadActive,
    md,
  ]);

  // FRONTMATTER-1: поля frontmatter для Properties-таблицы (сам блок убирается из рендера remarkFrontmatter,
  // строки тела не сдвигаются). У вложенных embed'ов frontmatter уже срезан (NoteEmbed) → таблицы нет.
  const fmFields = useMemo(() => {
    const fm = extractFrontmatter(source);
    return fm ? parseFrontmatterFields(fm.raw) : [];
  }, [source]);
  // В режиме шапки title/tags вынесены в kicker/заголовок → не дублируем их в Properties-таблице (md.fields
  // уже без них). Без шапки — полный набор полей (поведение embed/peek не меняем).
  const tableFields = masthead ? md.fields : fmFields;

  // S6-FIX1: МЕМОИЗИРУЕМ сам элемент `<ReactMarkdown>` по [body, components] (плагины — стабильные
  // модуль-константы). Так смена activeLine/popcard/любого «лёгкого» стейта родителя (или этого компонента)
  // НЕ перезапускает remark/rehype-пайплайн: React переиспользует тот же element, ReactMarkdown не ре-парсит.
  // Перерендерится ТОЛЬКО при реальной смене текста (body) или контракта оверрайдов (components). S3-collapse
  // НЕ инвалидирует это: collapsedSecs не входит в `components` (читается через SectionContext) — а Provider
  // оборачивает мемо-элемент СНАРУЖИ, поэтому смена value контекста ре-рендерит консьюмеров Section/h2 даже
  // сквозь memo-границу (контекст пробивает React.memo/мемоизированные subtree).
  const markdownEl = useMemo(
    () => (
      <ReactMarkdown
        remarkPlugins={REMARK_PLUGINS}
        rehypePlugins={REHYPE_PLUGINS}
        urlTransform={urlTransform}
        components={components}
      >
        {body}
      </ReactMarkdown>
    ),
    [body, components],
  );

  return (
    <EmbedContext.Provider value={embedCtx}>
      <div
        className={masthead?.reading ? `${styles.preview} ${styles.reading}` : styles.preview}
        ref={previewRef}
        // S7: страховка скрытия — курсор покинул всё превью (mouseleave не всплывает, срабатывает на
        // реальном выходе). Контейнер стабилен (не ремонтируется при ре-рендере ReactMarkdown), потому
        // надёжно гасит карточку, даже если узел-триггер пересоздался показом карточки.
        onMouseLeave={hidePopcard}
      >
        {masthead && (
          <header className={styles.docHead}>
            {md.kicker && <div className={styles.docKicker}>{md.kicker}</div>}
            {md.title && (
              <h1
                className={styles.docTitle}
                id={leadSlug ?? undefined}
                data-outline-line={md.h1Line ?? undefined}
              >
                {md.title}
              </h1>
            )}
            <div className={styles.docByline}>
              {masthead.mtime != null && (
                <span className={`${styles.chip} ${styles.datepill}`}>
                  <Clock size={12} aria-hidden /> {relTime(masthead.mtime, i18n.language)}
                </span>
              )}
              <span className={styles.chip}>{t('editor.metaWords', { count: words })}</span>
              <span className={styles.chip}>{t('editor.metaReading', { count: readingMinutes })}</span>
            </div>
          </header>
        )}
        {tableFields.length > 0 && <PropertiesTable fields={tableFields} onOpenTag={onOpenTag} />}
        {/* S3: контекст сворачивания секций — потребляют оверрайды h2 (кнопка) и section (класс). Provider
            СНАРУЖИ мемоизированного `markdownEl` (S6-FIX1): смена collapsedSecs ре-рендерит консьюмеров
            Section/SectionHeading через контекст, не инвалидируя дорогой remark/rehype-пайплайн. */}
        <SectionContext.Provider value={sectionState}>{markdownEl}</SectionContext.Provider>
        {/* AppendLine — только у top-level превью редактора (onAppendLine задан); embed/peek/доска не передают. */}
        {onAppendLine && fetchNotes && (
          <AppendLine onAppend={onAppendLine} fetchNotes={fetchNotes} />
        )}
        {/* S7: ховер-превью `.popcard` (вики/сноска). Fixed-узел — на layout не влияет; `pointer-events:none`
            не перехватывает клик/ховер. Ключ по rect+variant — новый mount на каждый ховер (рестарт анимации). */}
        {popcard && (
          <Popcard
            key={`${popcard.variant}:${popcard.rect.top}:${popcard.rect.left}`}
            rect={popcard.rect}
            content={popcard.content}
            variant={popcard.variant}
          />
        )}
      </div>
    </EmbedContext.Provider>
  );
}

/**
 * Hermes-8 S6: оборачиваем в `forwardRef`, чтобы GroupPane мог императивно раскрыть свёрнутую секцию
 * перед прыжком (revealLine). Совместимо с `React.lazy` (ref прокидывается через lazy-границу).
 * `displayName` — для DevTools/diagnostics (forwardRef иначе теряет имя).
 */
export const MarkdownPreview = forwardRef<MarkdownPreviewHandle, Parameters<typeof MarkdownPreviewImpl>[0]>(
  MarkdownPreviewImpl,
);
MarkdownPreview.displayName = 'MarkdownPreview';
