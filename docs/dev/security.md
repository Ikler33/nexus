# Безопасность каркаса (Ф0-12)

> §11, **AC-SEC-5**. Полная плагинная модель (broker, path-scoped permissions, iframe-изоляция,
> audit-log) — Фаза 2. Здесь — аудит каркаса: строгий CSP + минимальные capabilities + регресс.

## CSP (`tauri.conf.json` → `app.security.csp`)
Строгий, без `unsafe-inline` / `unsafe-eval`:
```
default-src 'self'; script-src 'self'; style-src 'self';
img-src 'self' asset: data: https://asset.localhost; font-src 'self' data:;
connect-src 'self' ipc: http://ipc.localhost; worker-src 'self' blob:;
object-src 'none'; base-uri 'self'; frame-ancestors 'none'
```
- React inline-стили (`style={{}}`) применяются через CSSOM (`.style`), CSP их НЕ блокирует
  (блокируются только HTML-атрибут `style` и `<style>`-элементы).
- CodeMirror 6 (style-mod) использует `adoptedStyleSheets` (constructable stylesheets) — не inline.
- Прод-CSS подключается через `<link>` (`style-src 'self'`).
- CSP enforce'ится в УПАКОВАННОМ приложении (asset-протокол), не в dev (Vite `devUrl`). Полная
  рантайм-проверка — на упаковке (Ф3/релиз). При проблемах со стилями — hash/nonce, НЕ `unsafe-inline`.

## Capabilities (`capabilities/default.json`)
Минимум: `core:default` + `dialog:default`. НЕТ широких `fs:`/`shell:`/`http:` прав — доступ к
vault идёт через СОБСТВЕННЫЕ команды (`read_file`/`write_file` через `resolve_vault_path`,
анти-traversal AC-SEC-1), а не через fs-плагин.

## Регрессия
`csp_and_capabilities_are_hardened` (`lib.rs`): CSP без unsafe-inline/eval, есть `object-src 'none'`;
в permissions нет `fs:`/`shell:`/`http:`. Падает, если ужесточение каркаса молча откатили.

## Сделано (Ф2 — рантайм плагинов)
- **Capability-broker** (реальная граница прав, §7.4): identity по capability-токену (не из payload),
  неотключаемый audit-log, path-scoped permissions (glob с deny-override), `ai:complete {local_only}`.
  Confused-deputy закрыт и в Rust, и на фронте (токен из привязки порта).
- **Sandbox-iframe** UI-вью (`allow-scripts`, opaque origin): Tauri-команды `vault.*` недостижимы из
  плагина — только через broker по `MessagePort` (AC-SEC-5). CSP без `unsafe-inline`/`unsafe-eval`.
- **Анти-SSRF для `net.fetch`** (AC-SEC-4): net-allowlist + `is_private_host` (приватные/loopback/
  link-local/metadata, напр. `169.254.169.254`, запрещены даже из allowlist), без следования редиректам.

## CI security-gate (V1.1, ревью B6 / AC-Q-5)
Отдельный job `security` в `.github/workflows/ci.yml` — проверки безопасности не тонут в общем
`cargo test`:
- **Supply-chain — `cargo-deny`** (`deny.toml`): advisories (RUSTSEC), лицензии (allow-list permissive,
  выверен по фактическому дереву — 12 лицензий, вкл. `CDLA-Permissive-2.0` для webpki-roots), баны
  дублей (warn), источники (только crates.io). `unmaintained = "workspace"` — неподдерживаемые крейты
  флагуем только среди ПРЯМЫХ зависимостей; глубокие транзитивные (gtk-rs/unic-* через Tauri — апстрим
  ещё на GTK3) вне нашего контроля. Уязвимости и **unsound** — блокируют.
- **Secret-scan — `gitleaks`** (`.gitleaks.toml`): дефолт-правила + allowlist путей с плейсхолдерами
  (доки/тесты) и заведомо-фейковых токенов (последовательные `0123…`). Реальные секреты в репозиторий
  не попадают: git-токен и LLM-ключи — в системном keychain (AC-SEC-3), не на диске.
- Гейт сразу сработал: cargo-deny вскрыл **RUSTSEC-2026-0008** (unsound `Buf`-deref в `git2` 0.19,
  прямая зависимость) → закрыт бампом `git2` 0.19→**0.20.4** (libgit2 1.8→1.9.4); git-тесты зелёные.

## Дальше (Ф2-доводка / Ф3)
- iframe-CSP **упакованного** app (`frame-src`/`child-src`, origin ассетов плагина) — проверяется
  `tauri build`; доверенный JS плагина в Worker (сейчас UI-JS в iframe).
- SSRF: DNS-rebinding (резолв домена + проверка адреса) — поверх литеральной проверки.
- **secret-scan коммитов** — сделано (gitleaks, см. выше); исключение кода плагинов из git (Ф3
  git-sync, AC-Б3); опц. at-rest шифрование.
- Выделенный прогон именно security-*тестов* отдельным required-шагом (а не общим `cargo test`) —
  нужна конвенция тегирования тестов; см. BACKLOG.
