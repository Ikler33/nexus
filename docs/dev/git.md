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

## Дальше — Ф3-3b
- pull/push (нужны сетевые фичи `git2`: https/ssh + credentials callback; решить хранение токена) +
  детект конфликтов + UI (диск vs грязный буфер редактора); pull нового/изменённого плагина (точнее —
  `config.json`) → состояние `needs-review` (AC-Б3-2, завязано на marketplace).
