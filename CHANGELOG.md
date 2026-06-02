# Changelog

Все значимые изменения проекта документируются в этом файле.
Формат основан на [Keep a Changelog](https://keepachangelog.com/ru/1.1.0/);
проект придерживается [Semantic Versioning](https://semver.org/lang/ru/).

## [Unreleased]

### Added — Фаза 0

- **Ф0-1 — Каркас (monorepo + Tauri 2 + CI).**
  - pnpm-workspace + Cargo workspace по §2 ARCHITECTURE: `apps/desktop/{src, src-tauri}`,
    заготовки `packages/`, `plugins/`, `scripts/`.
  - Tauri 2-приложение `nexus-desktop` с первой сквозной IPC-командой `app_version`;
    единый IPC-шов фронта `src/lib/tauri-api.ts` (контракт §4.1) — весь `invoke` только здесь.
  - Фронт: React 19 + Vite 6 + TypeScript (strict); базовые design-токены (DESIGN §2, light/dark).
  - Тулчейн качества: `tsc --noEmit`, ESLint 9 (flat config), Vitest 3 (+ Testing Library),
    `cargo fmt` / `clippy -D warnings` / `cargo test`.
  - CI (GitHub Actions): job `frontend` (typecheck · lint · test · build) и job `rust`
    (matrix Win/Mac/Linux: fmt · build · clippy · test).
  - Placeholder app-иконки: `scripts/gen-icon.mjs` → `cargo tauri icon` (полный платформенный набор).
  - Стартовый CSP + минимальные capabilities (`core:default`); строгий аудит — в Ф0-12 (AC-SEC-5).

  Закрытые гейты: **AC-Q-1**, **AC-Q-2**, **AC-Q-3** (зелёные сборка/тесты/линтеры).
