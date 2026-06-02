# Nexus

> Local-first knowledge base — Obsidian-форк (Tauri 2 + Rust + React) с глубокой интеграцией локального LLM/RAG.
> Vault до 50k+ `.md`, плагинная экосистема, i18n RU/EN, llama.cpp.

**Статус:** архитектура завершена (v1.1, отревьюена и доведена). **Фаза 0 в работе** — каркас (Ф0-1) готов: monorepo + Tauri 2-приложение, зелёные build/lint/test, CI. Прогресс — в `CHANGELOG.md`, инструкции по сборке — ниже («Разработка и сборка»).

## Навигация

| Путь | Что это |
|---|---|
| `CLAUDE.md` | Правила проекта (ADR, антипаттерны, стек, дисциплина доки) — Claude Code грузит автоматически каждую сессию |
| `docs/architecture/ARCHITECTURE.md` | **Источник истины.** Живой арх-план v1.1; раздел 0 — журнал решений (ADR-001…005) |
| `docs/architecture/ARCHITECTURE-v1.0-backup.md` | Снимок до правок по ревью |
| `docs/acceptance/ACCEPTANCE.md` | Критерии приёмки с ID (`AC-…`) — основа тестов |
| `docs/design/DESIGN.md` | UI/UX-контракт: токены, компоненты, состояния, a11y |
| `docs/reviews/REVIEW.md` | Мультиагентное ревью (12 блокеров + серьёзные) |
| `docs/reviews/RE-REVIEW.md` | Проверка: все блокеры закрыты в v1.1 |
| `docs/dev/` | Модульная дока — наполняется в ходе разработки (см. §8 промпта) |
| `prompts/DEV-PROMPT.md` | Kickoff-промпт для Claude Code (Фаза 0) |

## Порядок чтения

- **Новому участнику:** `ARCHITECTURE.md` (§0 ADR → §1 принципы → §4 слои) → `DESIGN.md` → `ACCEPTANCE.md`. Контекст «почему так» — в `reviews/`.
- **Для старта разработки:** открой `prompts/DEV-PROMPT.md` и следуй ему.

## Принятые решения (ADR, кратко)

1. Плагины — **JS-first + host-broker** (не WASM-first).
2. Безопасность — **capability-broker + path-scoped permissions**; код плагинов не в git.
3. БД — **rusqlite + write-actor** (не sqlx).
4. Граф — источник истины **SQLite** (petgraph опц. кэш).
5. AI — раздельные **Chat/Embedding** провайдеры, мультиязычный эмбеддер, cloud-fallback по opt-in.

Подробности и обоснования — в `docs/architecture/ARCHITECTURE.md` §0 и `docs/reviews/`.

## Старт разработки

1. Claude Code запускается из этого корня (`NEXUS/`) — `CLAUDE.md` подгружается автоматически (ADR, антипаттерны, дисциплина доки переживают `/clear`).
2. Стартовое сообщение — содержимое `prompts/DEV-PROMPT.md` (детальный метод + план Фазы 0).

## Разработка и сборка

**Требования:** Node ≥ 20 · pnpm ≥ 9 · Rust stable (через `rustup`) + `cargo-tauri` · системный webview (macOS — встроенный WKWebView; Linux — `webkit2gtk-4.1` и пр.; см. CI).

```bash
pnpm install                       # зависимости фронта (workspace)

# Приложение
pnpm dev                           # Tauri-приложение в dev-режиме (нативное окно + Vite HMR)
pnpm --filter @nexus/desktop dev   # только фронт на http://localhost:1420 (для браузерного превью)
pnpm build                         # продакшн-сборка фронта (tsc --noEmit + vite build)
pnpm build:app                     # сборка нативного бандла (tauri build)

# Верификация (каждый срез)
pnpm typecheck && pnpm lint && pnpm test          # фронт: tsc / eslint / vitest

cd apps/desktop/src-tauri
cargo fmt --all -- --check && cargo build && cargo clippy --all-targets -- -D warnings && cargo test
```

> Если `cargo` не найден в неинтерактивной оболочке — подгрузите окружение rustup: `source "$HOME/.cargo/env"`.

## Дисциплина документации

Цикл каждого среза: **реализация → тесты зелёные → обновить доку → следующий срез**. Баг-фиксы — всегда с регресс-тестом и правкой доки. Детали — `prompts/DEV-PROMPT.md` §8.
