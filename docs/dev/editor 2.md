# Редактор CodeMirror 6 (`src/components/editor`)

> Срез Ф0-5 (§4.1; DESIGN §6). Source-mode. Live Preview — отдельный эпик (С-22), НЕ в Ф0.

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
