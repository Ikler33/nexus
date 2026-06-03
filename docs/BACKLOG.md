# Backlog осознанно отложенного

> Единый реестр того, что **сознательно не сделано** и почему. Принцип «no silent caps»: если срез
> что-то урезал/отложил — пункт попадает СЮДА (а не только в `## Дальше` конкретной доки и текст
> коммита). Правило для будущих срезов: закрыл пункт → вычеркни; отложил новое → допиши.
>
> Колонки: **что** · **почему отложено** · **триггер** (что разблокирует / когда делать) · **источник**
> (AC / § ARCHITECTURE / дока). Статусы: ⏳ ждёт фазы · 🔬 нужен eval/замер · 🧩 нужен внешний кусок ·
> ✂️ refinement (не блокирует DoD).
>
> Сводка по DoD см. `docs/acceptance/ACCEPTANCE.md`; архитектурные решения — `docs/architecture`.

## Фаза 1 — RAG (текущая)

| Что | Почему отложено | Триггер | Источник |
|---|---|---|---|
| 🔬 **Реранкер** (cross-encoder поверх топ-N гибрида) | ADR-005 помечает опциональным; сервер :8082 — jina-эмбеддер кода, НЕ `/rerank`; изменение ранжирования нельзя вливать без eval | после **Ф1-10** (eval) + подтверждение `/rerank` на сервере | ADR-005, AC-EVAL-3, `search.md` |
| 🧩 **Реальный токенайзер чанкера** (сейчас `WordTokenizer`-placeholder; кириллица в токенах врёт ×1.5–2) | у эмбеддера нет endpoint `/tokenize`, считаем словами | появится tokenize у сервера / либа токенайзера | §6.1, `chunker.md`, `ai.md` |
| ✂️ Персистентная очередь индексации + дебаунс `save` usearch (сейчас save по чекпойнтам/событию) | mtime/hash-reconcile уже даёт базовую резюмируемость | при росте vault | §5.1, `indexer.md` |
| ✂️ Прогресс индексации **в UI** (сейчас только в логах N/M) | требует Tauri-события прогресса | Ф1-8 | AC-PERF-5, `indexer.md` |
| 🔬 Префильтр по **дате** + UI фильтров; калибровка `GRAPH_HOPS` и весов рангов | папка/тег уже есть; веса «с потолка» нельзя | Ф1-10 (eval) + UI | §6.2, AC-Б6-2, `search.md` |
| ✂️ Подсветка терминов в сниппете (FTS5 `snippet()`/`highlight`) | сейчас простой обрез чанка | — | `search.md` |
| ✂️ Параллельный начальный скан (сейчас последовательный; семафор к эмбеддеру готов) | хватает производительности на текущих объёмах | при больших vault | `indexer.md` |
| 🧩 Индикатор «☁ облако» + cloud-fallback chat-only (opt-in) | сейчас бейдж всегда «локально» | реализация cloud-fallback | ADR-005, `chat.md` |
| ✂️ Throttling рендера токенов + персист истории сессий (`ChatSession`) | MVP-чат держит сессию в памяти | при росте использования | DESIGN, `chat.md` |
| 🔬 **Suggest режим 2** (LLM-обоснование связи по действию) + калибровка `MIN_SCORE`/соседей | режим 1 (max-sim) закрыт; веса «с потолка» нельзя | Ф1-10 (eval) | §6, DESIGN, `suggest.md` |
| ⏳ Кэш `link_suggestions` (score/reason/dismissed/generated_at) + персист dismiss между сессиями | режим 1 считает на лету; dismiss — сессия | при росте vault | §5, `suggest.md` |

## Фаза 2 — плагины / broker / безопасность рантайма

| Что | Почему отложено | Триггер | Источник |
|---|---|---|---|
| ⏳ **Реальная загрузка кода плагина** из `.nexus/plugins/<id>/<entry>` (сейчас демо встроено в хост) + **iframe-CSP упакованного app** (`frame-src`/`child-src`, origin ассетов) + доверенный JS в **Worker** (сейчас UI-JS в iframe) | транспорт+sandbox готовы (Ф2-2b·4); загрузка/CSP/Worker — отдельный кусок | доводка Ф2 | ADR-001/002, §7.5, `plugins.md`, `security.md` |
| ⏳ Host-API плагинов: `ai.complete` (стрим ответа по порту) | embed/search/net.fetch сделаны; стрим чата по MessagePort (события host→plugin) — отдельным срезом | Ф2-3/доводка | §7.2, `plugins.md`, `ai.md` |
| 🔬 SSRF: DNS-rebinding для `net.fetch` (резолв хоста + проверка адреса) | литеральные приватные адреса + allowlist уже закрыты; резолв домена в приватный IP — глубже | при усилении | AC-SEC-4, `security.md` |
| ⏳ Миграции схемы `chat_*` / `link_suggestions` (FTS5/usearch нельзя `ALTER`) | не нужны до соответствующих фич | при их реализации | `db.md` |
| ⏳ **`registerEditorExtension` (AC-Б1-1)** — живое CodeMirror-расширение от плагина | CM-расширение не сериализуется через `MessagePort` → нужна модель **доверенного JS в main-контексте/Worker** (ADR), это другая граница исполнения, не sandbox-iframe | после ADR | AC-Б1-1, ADR-001, `plugins.md`, `editor.md` |
| ⏳ **Marketplace: подпись `id@version#sha256` + реестр + lifecycle** (install/update/rollback/uninstall, матрица §7.7) | ядро рантайма готово; дистрибуция/подпись/жизненный цикл — отдельная подсистема | после рантайма | AC-DOD-Ф2, AC-Б3-3, §7.7 |
| ⏳ **AC-Б3-1/2: код плагинов вне git** (auto-commit исключает `.nexus/plugins/**`; в коммит только `id@version#sha256`; pull → `needs-review`) | **зависит от слоя git-sync** | **Фаза 3** (git-sync) | AC-Б3-1/2, AC-SEC-3, `security.md` |
| ⏳ **Плагин-SDK + доки для сторонних разработчиков** (часть AC-DOD-Ф2) | host-API готов; нужен публикуемый SDK-пакет + dev-доки | после рантайма | AC-DOD-Ф2, `plugins.md` |

> **Итог Фазы 2:** ядро (рантайм плагинов: права→брокер→токены→sandbox-транспорт→host-API vault/ai/ui/net,
> AC-Б2 / AC-SEC-1/2/4/5(dev) / AC-I18N-7) — **закрыто**. Полное AC-DOD-Ф2 (editor-extensions, marketplace,
> SDK, git-exclusion) — отложено: одна зависимость кросс-фазная (AC-Б3 ↔ Фаза 3 git-sync), одна требует ADR.

## Фаза 3 / позже — sync, надёжность, доводка

| Что | Почему отложено | Триггер | Источник |
|---|---|---|---|
| ⏳ **i18n бэкенда** (Rust-ошибки, fluent/rust-i18n) | фронт-i18n закрыт (AC-I18N-1…5) | Ф3 | AC-I18N-6, `i18n.md` |
| ⏳ **git-sync** + конфликт «диск vs грязный буфер»; secret-scan коммитов | нет слоя синхронизации | Ф3 | `editor.md`, `security.md` |
| ⏳ Анти-SSRF валидация `*.url`; опц. at-rest шифрование (SQLCipher) | локальные доверенные эндпоинты на dev | Ф3 / релиз | §11, `security.md` |
| ⏳ Рантайм-CSP-проверка на упаковке | каркас CSP/capabilities закрыт | упаковка/релиз | AC-SEC-5, `security.md` |
| ✂️ Workspace: drag вкладок между группами, вертикальный сплит, персист раскладки | базовая модель групп/вкладок есть | — | DESIGN §3, `workspace.md` |
| ✂️ Калибровка `READ_POOL_SIZE` под нагрузку | дефолт 4 ок | Ф3 | `db.md` |

## Кросс-секционное / качество

| Что | Почему отложено | Триггер | Источник |
|---|---|---|---|
| 🧩 **Eval-гейт в CI без сервера** (кэш эмбеддингов golden → гонять без :8081) | живой гейт сейчас локально/с сервером; в CI только математика метрик | при настройке CI-eval | AC-EVAL-3, `eval.md` |
| 🔬 **Стоп-слова в FTS-запросе** (RU/EN): на малых vault слабый IDF → стоп-слова («на», «без», the) лексически цепляют неродственные заметки и через RRF теснят кросс-язычную семантику; на больших vault IDF давит сам | замер на реальном vault (smoke `live_real_vault_smoke` показал на 13 нотах) — если воспроизводится при росте, добавить стоп-лист/`weight` вектора при детекте кросс-языка | `search.md`, `eval.md` |
| ✂️ Резолв ссылок через `aliases` (frontmatter `aliases:`) | нет разбора frontmatter в JSON | при разборе frontmatter | `indexer.md` |
| ✂️ Rename как перемещение записи с сохранением `file_id` | сейчас delete+create | — | `indexer.md` |
| ✂️ Пагинация / бинарный канал для тяжёлых IPC | объёмы пока малы | при росте | §4.1 |

## Закрыто (история — для сверки, не для работы)
- **Ф3-3a — git-sync команды + UI + sync-lock** — `git_status`/`git_commit` (spawn_blocking + `git_lock`), `tauriApi.git` + `SyncPanel` (изменения, коммит, исход вкл. blocked-by-secrets), `view.sync`, i18n. Проверено в превью. Pull/push+конфликты — Ф3-3b.
- **Ф3-2 — git-sync коммит + secret-scan (AC-SEC-3)** — `commit_all` (add_all+update_all, авто-сообщение), `scan_secrets` (PEM/sk-/ghp_/AKIA/xox-, мало ложных); находка секрета → коммит блокируется. Команды/UI/sync-lock/pull-push — Ф3-3.
- **Ф3-1 — git-sync фундамент** — `GitSync` (git2/vendored libgit2): open/init, управляемый `.gitignore` (`.nexus/*` вне git, `!config.json` — фундамент AC-Б3-1/AC-SEC-3), `status`. Коммит+secret-scan (Ф3-2), pull/push+конфликты (Ф3-3) — далее.
- **`net.fetch` + SSRF-гард для плагинов (Ф2-3, AC-SEC-4)** — egress по net-allowlist + `is_private_host` (приватные/loopback/metadata запрещены), без редиректов. DNS-rebinding — в активном беклоге.
- **AI host-API для плагинов: `ai.embed` + `ai.searchSemantic` (Ф2-3)** — RAG из плагина через брокер (право `ai:embed`), `dispatch_ai` + read-лок `VaultContext`. Проверено в превью (аудит фиксирует `ai.searchSemantic`).
- **Плагинные i18n-namespace `plugin:<id>:<key>` (Ф2-3, AC-I18N-7)** — `ui.addTranslations` → i18next ns `plugin` (вложенно); `registerCommand` с `titleKey` → заголовок локализован и реагирует на смену языка. Проверено в превью (EN↔RU).
- **`registerCommand(source:'plugin')` (Ф2-3)** — плагин добавляет команду в палитру через брокер (право `ui:command`); двунаправленный транспорт (палитра → событие плагину → его обработчик). Проверено в превью.
- **Фронт-транспорт плагинов (Ф2-2b·4)** — sandbox-iframe + `MessagePort`-релей (`plugin-host.ts`), токен host-side, confused-deputy закрыт и на фронте; `tauriApi.plugins` + мок-брокер + `PluginsPanel` (демо + аудит-лог); `plugin_close_session` (отзыв). Проверено в превью.
- **Dispatch брокера vault read/list/write (Ф2-2b·3)** — `dispatch_vault` + `plugin_invoke(content?)`, scoped + defense-in-depth граница.
- **Capability-broker host-side + модель прав + токены + live-команды (Ф2-1/2-2a/2-2b)** — `permission.rs`/`broker.rs`, identity-по-токену, audit, path-glob с deny-override.
- **Windows-фикс анти-traversal** — `has_root()` в `resolve_vault_path*` (кросс-платформенно; поймано Windows-CI).
- **Виртуализация ленты чата** (DESIGN) — `@tanstack/react-virtual` в `ChatView` + умный автоскролл; проверено в превью.
- **Мультиязычный эмбеддер bge-m3 + AC-EVAL-6** — Ф1-12: bge-m3 @ :8083 (dim 1024), eval recall@8 1.0 (оба кросс-язычных кейса найдены). Риск ADR-005 снят.
- **Crash-reconcile usearch** (§5.1) — `reconcile_vectors` в `scan_vault` (ночь, после Ф1-10).
- **AC-Б6-2** префильтр до KNN — был отложен в Ф1-6, закрыт в доработке Ф1-6+ (usearch `filtered_search`).
- **Граф как 3-й ранг RRF (без +0.2)** — отложен в Ф1-6, закрыт в Ф1-6+ (REVIEW С-4).
- **Dedup overlap** — отложен в Ф1-6, закрыт в Ф1-6+.
