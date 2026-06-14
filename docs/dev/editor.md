# Редактор CodeMirror 6 (`src/components/editor`)

> Срез Ф0-5 (§4.1; DESIGN §6). Source-mode + read-only preview (#20). **Live Preview** (inline-правки
> с рендером на месте) — отдельный пост-v1 эпик (С-22).

## Контракт CM6↔React (`Editor.tsx`)
- `EditorView` создаётся ОДИН раз (`useEffect([])`), уничтожается в cleanup → двойной mount
  StrictMode безопасен (guard по `viewRef`).
- Смена файла (`path` prop) → замена документа ОДНИМ `dispatch` (view НЕ пересоздаётся).
  Транзакция помечается аннотацией `externalSync` ⇒ не считается пользовательской правкой
  (нет ложного `dirty`). Регресс закрыт тестом.
- Колбэки (`onChange`/`onSave`/`onOpenLink`/`getNotes`) читаются из ref → всегда актуальны без
  перестройки расширений. Сохранение — `Mod-s` → `onSave`.

## Расширения (`extensions.ts`)
- `markdown()` + `syntaxHighlighting(defaultHighlightStyle)` — подсветка source-mode.
- Декорации `[[wikilink]]` / `![[embed]]` / `#tag` (ViewPlugin поверх видимой области; классы
  `.cm-wikilink` / `.cm-tag`, цвета — токены `--color-link` / `--color-tag`).
- Клик по `[[wikilink]]` → `onOpenLink(target)` (`posAtCoords` + матч по строке; `normalizeTarget`
  срезает `#heading` / `|alias`).
- Автокомплит имён заметок внутри `[[…` (источник — `getNotes()` → команда `list_notes`).

## Read-only preview (`MarkdownPreview.tsx`, #20)
- Переключатель **Исходник/Просмотр** — в панели вкладок (`GroupPane`, кнопка-книга), только для `.md`;
  режим локален на группу-сплит. В preview-режиме вместо `Editor` рендерится `MarkdownPreview` от
  `active.doc` (read-only; правки — в source-режиме).
- `react-markdown` + `remark-gfm` (таблицы, таск-листы, strikethrough). **CSP-safe**: сырой HTML НЕ
  рендерится (`rehype-raw` не подключён → нет `dangerouslySetInnerHTML`); `urlTransform` пропускает
  `http(s)`/`mailto`/`tel`/относительные + кастомные nexus-схемы, режет `javascript:`/`data:`.
- `[[wikilink]]` и `#tag` — remark-плагин `remarkNexus` (`lib/markdown/remarkNexus.ts`): на mdast-уровне
  заменяет в `text`-узлах на `link` с URL `nexus-wikilink:<encoded>` / `nexus-tag:<encoded>` (внутрь
  `code`/`inlineCode` НЕ лезет). `MarkdownPreview` ловит их в `components.a` по префиксу href: wikilink →
  кликабельная навигация (`onOpenLink`, цель `decodeURIComponent`), tag → чип-`span`.
- **Математика** `$$…$$` (#4) — `remark-math` (`singleDollarTextMath:false` — одиночный `$` отдан под
  валюту, иначе суммы `$5…$10` ломались бы) + `rehype-katex` с `output:'mathml'`: чистый нативный `<math>`
  БЕЗ inline-стилей и без шрифтов KaTeX → строгий CSP не трогаем. `lib/markdown/rehypeKatexCsp.ts` снимает
  единственный инлайн-`style`, что KaTeX даёт на битом LaTeX (`.katex-error`) и `\fcolorbox`. Рендер —
  нативный MathML вебвью (macOS WebKit — из коробки; бандл math-шрифта для Win/Linux — BACKLOG).
- **Vault-картинки** (IMG-1) — `attachments/` + `![](…)` через data-URL (CSP уже разрешал `img-src data:`).
- **Отложено**: Mermaid-диаграммы (inline-стили под CSP). Глобальная команда/хоткей переключения
  (Ctrl+E) — потребует подъёма режима в стор (сейчас кнопка на пане).

## Данные (vault store + команды)
- Команды Rust: `read_file` / `write_file` (write-safe `resolve_vault_path_for_write`), `list_notes`.
- Стор: `activeFile{path,content}`, `dirty`, `notes`; `openFile`, `openLink` (через `resolveLink`),
  `setActiveContent` (правки → dirty), `saveActiveFile`. Клик по файлу в дереве → `openFile`.

## Тесты
- `extensions` (normalizeTarget), `Editor` (рендер + замена документа + регресс `externalSync`),
  стор (openFile/openLink/resolveLink), FileTree (открытие файла).

## Дальше
- Несколько буферов / вкладок / сплитов — Ф0-9 (сейчас один активный файл).
- Live Preview (скрытие разметки вне строки курсора, рендер embeds/таблиц/LaTeX) — отдельный эпик.
- Подтверждение «диск vs грязный буфер» при внешнем изменении файла — с git-sync (Ф3).
