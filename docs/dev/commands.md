# Реестр команд + палитра + keymap (`src/lib/commands*`, `components/command`)

> Срез Ф0-8 (§4.6). Единый реестр: ядро и плагины (Ф2) регистрируют команды одним путём.

## Реестр (`lib/commands.ts`)
- `commands.register(cmd) -> Disposable`; `list` / `get` / `run` / `subscribe`.
- `Command { id, title, source: core|plugin|user, defaultKey, run }`.
- Хоткеи: `normalizeCombo` (`mod`→⌘/Ctrl, фиксированный порядок модификаторов), `eventToCombo`,
  `formatCombo` (для UI).
- `resolve(combo)`: **пользователь > плагин > ядро**; `setUserKey` — пользовательский ремап.

## Палитра (`components/command/CommandPalette.tsx`)
- Cmd/Ctrl+P (через keymap → команда `palette.open`). Фильтр по названию; ↑/↓/Enter/Esc, клик.
  Открытость хранится в `useUIStore`. Keyboard-first (DESIGN §1/§9a).

## keymap (`hooks/useKeymap.ts`)
- window `keydown` (только с модификатором) → `commands.resolve` → `run`. Комбинации без
  модификатора (Esc/стрелки) обрабатывают сами компоненты.

## Команды ядра (`lib/commands-core.ts`)
- `palette.open` (mod+p), `vault.open` (mod+o), `file.save`. Регистрируются в App на mount.

## Тесты
- Реестр: register/run/dispose; `normalizeCombo`/`eventToCombo`; приоритет user>plugin>core.
- Палитра: открытие, фильтр, запуск по Enter, закрытие по Esc.

## Дальше
- Плагинный `registerCommand` (Ф2) — тот же `commands.register` с `source: 'plugin'`.
- Context-menu поверх реестра; UI настройки keymap; уведомление о перекрытии хоткеев.
