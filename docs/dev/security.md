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

## Дальше (Ф2/Ф3)
- Capability-broker (реальная граница прав), MessagePort-identity, неотключаемый audit-log,
  path-scoped permissions, `ai:complete {local_only}`.
- iframe-изоляция UI-вью; Tauri-команды vault.*/git.* недостижимы из iframe (только через broker).
- Анти-SSRF валидация `*.url`; secret-scan коммитов (Ф3 git-sync); опц. at-rest шифрование (SQLCipher).
