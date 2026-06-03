# git-sync (`src-tauri/src/git/`) — Фаза 3, §8

> Vault как git-репозиторий. **Core module** (не sandbox-плагин, §8/ADR). На `git2` (vendored libgit2 —
> кросс-платформенно, без системной зависимости). Весь libgit2-I/O синхронный → из Tauri-команд только
> в `spawn_blocking`.

## Сделано — Ф3-1 (фундамент)
- `GitSync::open_or_init(root)` — открывает репозиторий или `git init` (включить синк = сделать vault репо).
- `ensure_gitignore()` — идемпотентно добавляет **управляемый блок** (по маркеру `# >>> nexus (managed) >>>`):
  - `.nexus/*` — внутреннее Nexus (индекс/векторы/БД, секреты `local.json`, **код плагинов**) **НЕ в git**
    → фундамент **AC-Б3-1** (код плагина не доставляется через git) и **AC-SEC-3**;
  - `!.nexus/config.json` — декларация установленных плагинов (`id@version#sha256`) **синхронизируется**;
  - пользовательские правила сохраняются, блок не дублируется.
- `status()` — изменённые/новые/удалённые файлы рабочего дерева, **без игнорируемых** (gitignore в силе).
  Пути относительные, разделитель `/`.

## Сделано — Ф3-2 (коммит + secret-scan)
- `commit_all()` — стейджит все не-игнорируемые изменения (`add_all` + `update_all` для удалений),
  **сканирует их содержимое на секреты** (AC-SEC-3): находка → коммит НЕ делается (`BlockedBySecrets`);
  иначе коммит с авто-сообщением. Идемпотентно (`NothingToCommit`). Подпись из git-config, иначе дефолт.
- `scan_secrets(text)` — высокоточные форматы: PEM private key, `sk-…` (OpenAI), `ghp_…`/`github_pat_`
  (GitHub), `AKIA…` (AWS), `xox…` (Slack). НЕ детектит общие «high-entropy» строки → мало ложных.
- Авто-сообщение: `Vault sync: +N new, ~M changed, -K deleted`.

## Тесты (5)
Ф3-1 (3): gitignore исключает `local.json`/`plugins/`, оставляет `config.json`+заметки; идемпотентность;
open существующего. Ф3-2 (2): детект форматов секретов без ложных (URL/текст); коммит → nothing →
блокировка секрета (не закоммичен).

## Сделано — Ф3-3a (команды + UI + sync-lock)
- Tauri-команды `git_status` / `git_commit` (`commands/git.rs`): libgit2 в `spawn_blocking`, под
  **sync-локом** `AppState::git_lock` (tokio Mutex — один git-вызов за раз). Репозиторий открывается
  per-вызов (git2 `!Send`); `ensure_gitignore` гарантируется на каждом вызове.
- Фронт: `tauriApi.git` (status/commit) + мок (`lib/mock/git.ts`); панель `SyncPanel` (изменения с
  бейджами A/M/D/R, кнопка коммита, исход committed/nothing/**blocked-by-secrets** с файлами+строками),
  кнопка/команда `view.sync`, i18n RU/EN. Проверено в превью.

## Сделано — Ф3-3b-1 (credentials в keychain, AC-SEC-3)
`git/creds.rs` (на `keyring` 3): токен доступа к remote — в системном keychain ОС (macOS Keychain /
Windows Cred Manager / Linux Secret Service через zbus, pure-Rust), **на диск не пишется**.
`set_token`/`get_token`/`delete_token`/`has_token` (`service=nexus-git`, `account=<путь vault>`). Команды
`git_set_token`/`git_clear_token`/`git_has_token` (`spawn_blocking`) + `tauriApi.git` + мок. Тесты: guarded
роундтрип (`#[ignore]`, реальный keychain) + мок-токен на фронте. Используется в pull/push (Ф3-3b-2).

## Сделано — Ф3-3b-2 (remote + pull/push по https)
git2 с `https` + **vendored-openssl** (кросс-платформенно). `set_remote`/`get_remote` (origin); `push`
(текущая ветка); `pull` = fetch + merge-analysis → `up-to-date` / `fast-forward` (применяется) /
`merge-required`. credentials-callback берёт токен из keychain (Ф3-3b-1) как https-пароль. Команды
`git_set_remote`/`git_get_remote`/`git_sync` (pull-ff→push под sync-локом) + `tauriApi.git` + мок.
Тесты: remote set/get/overwrite (юнит); push/pull — сеть, не юнит-тестятся.

## Дальше — Ф3-3b-3 (UI + конфликты)
- UI настройки remote (URL + токен в keychain) + кнопка sync + индикатор подключения.
- Разрешение конфликтов при `merge-required` (диск vs грязный буфер редактора).
- pull изменённого `config.json` плагина → `needs-review` (AC-Б3-2, завязано на marketplace). **Закрывает AC-Б3.**
