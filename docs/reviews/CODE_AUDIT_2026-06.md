# Дотошный код-аудит Nexus — 2026-06-15

> Мультиагентный аудит (21 финдер по подсистемам + cross-cutting линзы → независимая верификация
> КАЖДОГО замечания по коду → дедуп → ранжирование). 132 агента, 102 подтверждённых замечания →
> **88 уникальных после дедупа: 1 critical · 17 major · 33 minor · 38 nit.** Контроль ложных
> срабатываний: каждое замечание подтверждено скептиком против реального кода (отклонённые отброшены).
>
> Очередь исправления — §«Порядок» внизу. Статусы: `[ ]` открыто · `[x]` закрыто (PR#) · `[-]` отложено/отсеяно.
>
> **СТАТУС 2026-06-15: проработка завершена.** Все autonomous-safe находки закрыты батчами B1–B14 (PR #240–253) поверх #231–238. Итог — внизу (§«Итог»). Остаток отложен в `docs/BACKLOG.md` с обоснованием (схема/перф-риск/honesty-хвосты).

## Доминирующие оси риска
- **Безопасность/egress (SSRF)** — корневой узел: plugin-egress + `is_private_host` (IPv4-mapped IPv6); вторично — границы ФС (vault path-traversal, `.nexus`, git).
- **Целостность данных / durability** — `vault::atomic_write` существует, но НЕ применён в 4+ местах; потеря контента при курации/регенерации/захвате.
- **Корректность/стейт-машины** — гонки фронт-сторов без эпох-гарда (повторяющийся паттерн).
- **Честность/деградация** — молчаливые `catch`, врущие индикаторы прогресса/состояния.
- **Пересечения фич** — курация (rename/move/delete) не синхронизирует сателлитные хранилища (история/starred/recents/navHistory).
- **a11y** — модалки без focus-trap/Esc, reading-Esc конфликт.

---

## CRITICAL (1)

- [x] **SSRF плагин-egress + DNS-rebinding** (кластер из 6 находок) — **ЗАКРЫТО (PR #231).** `embedded_ipv4` нормализует ВСЕ туннельные v4-формы (mapped/NAT64 64:ff9b/6to4 2002/compatible) в `is_private_host`+`blocks_cloud_metadata`; +CGNAT 100.64/10; plugin `net.fetch` резолвит→`guard_fetch_ips`(все IP)→пин IP (анти-rebinding)+scheme-allowlist http(s). Двойной security-ревью (1-й поймал NAT64/6to4/CGNAT-обход → расширил → 2-й: ship). Исходно: SSRF плагин-egress + DNS-rebinding (кластер из 6 находок). Корень — `is_private_host` (V6-ветка `plugin/permission.rs:256-263`) не нормализует IPv4-mapped IPv6 (`::ffff:192.168.x`/`::ffff:169.254.169.254`) → классифицирует как НЕ-приватный; затрагивает И core news/web DNS-гард (`news/fetch.rs:check_resolved_ips`, `websearch/search.rs`), И плагинный `net.fetch` (`commands/plugin.rs:140-170`), который вдобавок НЕ резолвит/не пинит IP и НЕ зовёт `blocks_cloud_metadata`. Радиус: эксфильтрация cloud-metadata-кредов (IMDS 169.254.169.254) + доступ во всю LAN/loopback (вкл. боевой LLM-сервер) с возвратом тела. **Fix:** (1) `to_ipv4_mapped()`-нормализация в `is_private_host` V6-ветке; (2) plugin `net.fetch` → резолв+пин IP через `check_resolved_ips`-модель + `blocks_cloud_metadata`; (3) тесты `::ffff:`-кейсов.

---

## MAJOR (17)

- [x] **delete/rename: гард служебных путей обходится через `..`** — **ЗАКРЫТО (PR #232).** `points_into_reserved` компонентная проверка ПОСЛЕ канонизации (delete + оба конца rename); `to_abs` канонизируется (parent), `..` схлопывается. Исходно: живой `nexus.db`/`.nexus` можно отправить в корзину.
- [x] **rename/move не переносит `.nexus/history/<rel>/`** — **ЗАКРЫТО (PR #232).** `vault::history::move_history` переносит каталог истории (best-effort) после `fs::rename`; тест move+orphan-clear. Исходно: история недоступна по новому пути (ломало SAFE-5/6).
- [x] **Молчаливое усечение векторов** (zip-truncation) — **ЗАКРЫТО (PR #235).** Контракт эмбеддера `AiError::CountMismatch` (`out.len()!=inputs.len()` в `embed_raw`) + инвариант `vectors.len()==new_chunk_ids.len()` перед usearch-upsert + guard в reconcile. Тест truncating-embedder.
- [-] **get_backlinks дубли** — **ОТСЕЯНО (false-positive, верификация B4).** `get_backlinks` намеренно возвращает ВСЕ ссылки (вкл. дубли из одного источника) — контракт BacklinksBar (счётчик + контекст/line_number); `GROUP BY` потерял бы occurrences. Self-loop из выдачи убран отдельно (#243). Исходно: повторное `[[упоминание]]` = отдельная строка.
- [x] **graph_rank неограниченные IN** — **ЗАКРЫТО (PR #236).** Оба `IN` через `graph::collect_in_chunks` (стал `pub(crate)`, чанк ≤900): BFS — две одиночные стороны рёбер (`source`/`target`), chunks-query чанкуется. Тест super-hub N=1000/hops=2.
- [x] **Вотчдог-таймаут оставляет джобу в running навсегда** — **ЗАКРЫТО (PR #238).** `requeue_running` (running→pending) в Err-ветке таймаута тика — как crash-recovery на старте воркера.
- [x] **git force-checkout на грязном дереве** — **ЗАКРЫТО (PR #240).** `ensure_clean_tree()` блокирует pull (FF + apply_merge) при Modified/Deleted/Renamed в рабочем дереве (untracked не мешают) → `GitError::DirtyTree` вместо тихой потери; UI-сообщение «commit/stash перед pull». Тесты dirty-blocks-pull / untracked-ok. Исходно: молчаливая потеря незакоммиченных правок.
- [x] **plugin vault.writeFile не запрещает `.nexus/`** — **ЗАКРЫТО (PR #232).** `is_escaping` блокирует сегменты `.nexus`/`.git` (case-insensitive) ДО scope — все vault-методы плагина (read/write/list) через `check_path`. Исходно: плагин с `vault:write['**']` перезаписывал секреты/БД/код плагинов.
- [x] **⌘L toggleTask портит нумерованные таски** — **ЗАКРЫТО (PR #237).** `transformTaskLine` зеркалит `TASK_LINE_RE` по набору маркеров (`[-*+]`/`\d+[.)]`) и СОХРАНЯЕТ нумерованный маркер при реконструкции (`1. [ ] x`↔`1. [x] x`); буллеты `-*+` по-прежнему нормализуются в `- `. +2 теста.
- [x] **dailyNote/quickThought перезаписывают файл при ЛЮБОЙ ошибке чтения** — **ЗАКРЫТО (PR #233).** Переведены на `openOrCreateDaily`/`openOrCreateInbox` с `fileHash`-проверкой существования (null только при отсутствии файла); дневник унифицирован с ⌘⇧D (`Journal/`). `components/home/HomeView.tsx`, `lib/daily.ts`.
- [-] **Регенерация удаляет обмен из БД ДО переспроса** — **НЕ БАГ (отсеяно адверсариально, аудит-ревью PR #233).** `stores/chat.ts:329-342`: ждёт персист прошлого обмена, ре-чекает дрейф ленты, чистит БД (`deleteLastExchange`) только для персистнутого **не-ошибочного** обмена (`sid != null`), текст вопроса сохраняется в `question` и переспрашивается. Потери нет.
- [x] **Quick-capture ⌘⇧N пишет Inbox.md мимо открытого грязного буфера** — **ЗАКРЫТО (PR #233).** `appendCapture` теперь буфер-aware: открыт Inbox → `updateBufferDoc`, иначе атомарный диск. `lib/daily.ts`.
- [x] **Избранное не переживает rename/delete** — **ЗАКРЫТО (PR #247).** `vault.deleteFile`→`dropStarsUnder`, `vault.renameFile`→`starred.rename` (точный путь + дети каталога, префикс-safe); +3 теста. Исходно: звёзды осиротевали, `starred.rename` был мёртвым кодом.
- [x] **reloadWidget без try/catch** — **ЗАКРЫТО (PR #233).** Обёрнут в try/catch со снятием `generating[key]` при ошибке (как `refreshWidget`). `stores/home.ts`.
- [x] **plugin vault.writeFile неатомарен** — **ЗАКРЫТО (PR #234).** `vault::atomic_write_io` в `spawn_blocking` (окно повреждения .md устранено).
- [x] **Конфиги egress-consent неатомарны** — **ЗАКРЫТО (PR #234).** Единый `vault::atomic_write_io` (tmp→fsync→rename) во всех 4 конфиг-врайтерах + news/chat-экспорт (7 сайтов). Тест no-leftover-tmp.
- [x] **Web-поиск настройки молча проглатывают ошибку записи** — **ЗАКРЫТО (B16).** `persist` web-конфига получил `.catch` → `settings.web.saveError` (как egress-блок); +тест. Заодно `SyncPanel.saveRemote` (нит) — error-состояние вместо пустого catch.

---

## MINOR (33) — кратко (title · location · fix)

- [x] traversal истории: `list_versions/read_version` не валидируют `rel` · `commands/vault.rs` · **#240** (`validate_history_path`).
- [-] осиротевшая история/корзина не GC при delete (рост диска) · `commands/vault.rs:713-738` · **ОТЛОЖЕНО → BACKLOG** (best-effort GC history + ретенция корзины; не потеря данных, рост диска).
- [x] TOCTOU снапшота истории (две записи на одну заметку) · `vault/history.rs` · **#240** (O_EXCL-цикл).
- [-] reconcile_vectors не чистит orphan-векторы · `indexer/rag.rs:144-209` · **ОТЛОЖЕНО → BACKLOG** (риск usearch v2 `all_keys()` API — может снести лишнее; нужен guard removed>0 + лог).
- [-] mtime-шорткат пропускает правку той же секунды при равном размере · `indexer/mod.rs:140` · **ОТКЛОНЕНО (адверсариально, B5):** хеш-сверка при совпадении mtime+size = чтение+хеш ВСЕХ файлов каждый скан (перф-регрессия, отменяет cross-file-эпик); редкий same-second+same-size edit ловит watcher fs-event.
- [x] несбалансированный `[[` матчит через документ · `parser/mod.rs` · **#245** (поиск `]]` в пределах строки).
- [-] Atom: второй date-элемент конкатенируется → published_at=0 · `news/parse.rs` · **ОТСЕЯНО (false-positive):** guard `if date.is_empty()` уже предотвращает конкатенацию; фикстура `willison_atom.xml` даёт валидные ts.
- [x] HN: двойная keyword-фильтрация отбрасывает совпавшие без story_text · `news/run.rs` · **#253** (HN не фильтруем повторно — Algolia уже отфильтровал).
- [x] Дайджест: контент заметок в LLM без injection-маркеров · `digest/mod.rs` · **#244** (`injection_marker`).
- [x] фид не-UTF8 → жёсткий отказ · `news/fetch.rs` · **#244** (`from_utf8_lossy`).
- [x] self-backlink `[[A]]` в A.md + self-loop ребро · `graph/mod.rs` · **#243** (`source_id != target_id`).
- [x] get_full_graph: степень завышена дублями/self · `graph/mod.rs` · **#243** (self исключён в обеих UNION-ветках degree).
- [-] contradictions: пустой результат → `should_generate` вечно-true · `contradictions/mod.rs:304-316` · **ОТЛОЖЕНО → BACKLOG** (нужен отдельный `last_run` в схеме → миграция; не баг, лишний LLM-прогон).
- [-] contradictions seed без `has_ready_job` → дубль джобы · `commands/vault.rs:390-399` · **ОТЛОЖЕНО → BACKLOG** (`reschedule_if_absent`; дубль-джоба, эффективность).
- [x] relation_reasons: кэш-хит игнорит пустое объяснение → LLM пере-вызов · `relation_reasons/mod.rs` · **#242** (убран `&& !expl.is_empty()` из кэш-хита).
- [x] hash_snippet на `DefaultHasher` (нестабилен по версиям Rust) · `contradictions/mod.rs` · **#242** (blake3).
- [x] contradictions::list отдаёт пары удалённых заметок · `contradictions/mod.rs` · **#242** (EXISTS-фильтры `is_deleted=0`).
- [x] recurring dead-джоба не переназначается · `scheduler/mod.rs` · **#245** (re-arm `reschedule_if_absent` при терминальной неудаче).
- [x] write_actor умирает при первой панике · `db/write_actor.rs` · **#241** (`catch_unwind` вокруг job).
- [x] read_pool теряет conn при панике замыкания · `db/read_pool.rs` · **#241** (`catch_unwind`, conn всегда возвращается в пул).
- [x] миграции: нет потолка версии (downgrade молча работает) · `db/migrations.rs` · **#241** (`Err` при `current > latest`).
- [x] suggest `dismissed` кэш не чистится при смене vault · `stores/suggest.ts` · **#248** (`clearDismissed()` в `openVault`).
- [x] home `error`/`loading` не читаются HomeView (мёртвые поля) · `HomeView.tsx` · **#252** (error-баннер + loading-хинт).
- [x] news.load без эпох-гарда · `stores/news.ts` · **#252** (epoch-счётчик, стейл-ответ отброшен).
- [x] loadSession проверяет streaming только на входе · `stores/chat.ts` · **#251** (перепроверка `streaming` после await).
- [x] inline `data:`-картинка вычищается urlTransform · `MarkdownPreview.tsx` · **#246** (разрешён ТОЛЬКО `data:image/` — не blanket `data:`, анти-XSS).
- [x] OutlineBar пере-извлекает заголовки на каждый символ · `OutlineBar.tsx` · **#250** (`useDeferredValue`).
- [x] чат onEvent без эпох-гарда (поздние token после stop) · `stores/chat.ts` · **#251** (per-message epoch-гард: событие только пока replyId streaming).
- [x] radiogroup режима без стрелок/roving tabindex · `ChatView.tsx` · **#249** (roving tabindex + Arrow-keys).
- [x] drag-ресайз панели не чистит листенеры при unmount · `AiPanel.tsx` · **#250** (AbortController + unmount-cleanup).
- [x] disclosure-переполнение чистит ВСЕ раскрытия · `ChatView.tsx`/`chat.ts` · **#251** (`DisclosureMap` LRU-кап 600 вместо clear).
- [x] AiPanel/bucketOf без тестов · `AiPanel.tsx` · **#250** (AiPanel.test.tsx — drag-cleanup; `bucketOf` — остаётся как nit).
- [x] модалки Digest/Contradictions/Settings без focus-trap/Esc · `*Panel.tsx` · **#249** (`useFocusTrap`).
- [x] reading-Esc закрывает чтение вместо модалки · `App.tsx` · **#249** (+флаги conflict/goals/tasks/inbox/digest/contradictions/tweaks).
- [x] FileTree active не клампится при сжатии → битый aria-activedescendant · `FileTree.tsx` · **#249** (кламп через useEffect).
- [-] NewsView offline-баннер fire-once · `NewsView.tsx:93-101` · **ОТЛОЖЕНО → BACKLOG** (нужен egress-event или поллинг; маргинально — офлайн-фетч и так даёт видимую ошибку).
- [-] ремап хоткея не снимает старый дефолт (ghost binding) · `lib/commands.ts` · **ОТСЕЯНО (false-positive):** `remap()` удаляет старые user-биндинги, `resolve()` приоритизирует user>plugin>core; дефолты-как-фолбэк намеренны.
- [x] remarkNexus подсвечивает кириллический #тег, бэкенд не создаёт · `lib/markdown/remarkNexus.ts` · **#246** (теги только ASCII, как `is_ascii_alphabetic`).
- [x] relTime показывает «1 минуту назад» для будущего · `lib/time.ts` · **#246** (знаковый diff).
- [-] candidate_pairs N+1 по read-пулу · `contradictions/mod.rs:87-114` · **ОТЛОЖЕНО → BACKLOG** (батч-выборка; перф, не корректность).
- [-] GraphView перерисовывает весь SVG на тик · `GraphView.tsx` · **ОТСЕЯНО (stale-already-fixed):** уже починено в #148 (render-throttle каждый 3-й тик ~20fps, drag 60fps).
- [x] кэши без cap (startingQuestions/relations) · `startingQuestionsCache.ts` · **#250** (`CACHE_CAP=200` FIFO; relations-cache — остаётся как nit).
- [x] news/export пишутся неатомарно (новые файлы → minor) · `commands/news.rs`, `chat_sessions.rs` · **#234** (`atomic_write_io`, в durability-батче).
- [x] reset() чистит recents только в памяти, не localStorage · `stores/workspace.ts` · **#248** (`saveRecents([])`).
- [x] ConflictResolver без Esc + вне reading-Esc списка · `ConflictResolver.tsx` · **#249** (`useFocusTrap` + `conflictOpen` в App-гарде).
- [-] capture/templates/versions вне TRAP_OVERLAYS_CLOSED (стек 2 модалок) · `stores/ui.ts` · **ОТСЕЯНО (false-positive):** capture/templates/versions НЕ используют `useFocusTrap` (свои Esc/autoFocus) → стека focus-trap нет (явно ограничено в #212).
- [x] moveTab не ремапит navHistory groupId · `stores/workspace.ts` · **#248** (ремап `fromGroupId→toGroupId` записей перемещённого пути).
- [x] SyncPanel commit/saveRemote молча глотают ошибку · `SyncPanel.tsx` · **#252** (commit → `CommitResult`) + **B16** (`saveRemote` → error-состояние `remoteError`).
- [x] упавшие при скане файлы молча выпадают, прогресс 100% · `indexer/mod.rs` · **#245** (счётчик failed + warn).

## NIT (38) — фоновая чистка по мере касания модулей
docstring-дрейф (`goals/mod.rs:5`), мёртвый код (`chat.ts` clear() no-op, mock manual), edge-парсеры (frontmatter-отступ, `C#` heading, несбалансир. `[[`), honest-логирование (watcher Err, app_config_dir), perf-гигиена (EgressAudit cap, chunk_file_meta IN дедуп), a11y (dirty-tab close), и пр. — полный список в исходном отчёте workflow. Группировать по подсистеме при касании.

---

## Порядок исправления (фикс-батчи)
1. ✅ **SSRF-кластер plugin-egress + is_private_host** (CRITICAL) — **PR #231.** Эксфильтрация секретов/LAN.
2. ✅ **delete/rename path-traversal через `..`** (major) — **PR #232.** Security-граница ФС.
3. ✅ **`.nexus` писабелен/читаем плагином** (major) — **PR #232** (один проход с #2).
4. ✅ **Атомарность конфигов + plugin/news/export/vault.writeFile** (major+minor) — **PR #234** (`vault::atomic_write_io`, 7 сайтов).
5. ✅ **rename переносит `.nexus/history`** (major) — **PR #232** (`move_history`). GC корзины — в #16.
6. ✅ **Потери данных в UI: dailyNote/quick-capture + reloadWidget** (2×major+minor) — **PR #233.** (regenerate — не баг, отсеян.)
7. ✅ **git force-checkout на грязном дереве** (major) — **PR #240** (B1: `ensure_clean_tree` + history TOCTOU/traversal).
8. ✅ **zip-усечение векторов** (major) — **PR #235** (`CountMismatch`+инвариант). (scan silent-drop → B5 #245; reloadWidget — в #233.)
9. ✅ **graph_rank unbounded IN** — **PR #236** (`collect_in_chunks` pub(crate)). chunk_file_meta — уже чанкуется.
10. ✅ **toggleTask нумерованные** (major) — **PR #237.** get_backlinks дубли — отсеяно (occurrences-контракт BacklinksBar).
11. ✅ **Планировщик: watchdog requeue** (major) — **PR #238.** recurring re-arm → B5 #245; contradictions seed → BACKLOG.
12. ✅ **Cross-feature синхронизация: starred/recents/navHistory/suggest при курации/смене-vault** — **PR #247/#248** (B9 starred · B8 recents/moveTab-navHistory/suggest-dismissed).
13. ✅ **Фронт-гонки эпох-гард + honesty** — **PR #251/#252** (B12 onEvent/loadSession/disclosure-LRU · B13 news-epoch/home-error/SyncPanel-commit). web-save-swallow → BACKLOG.
14. ✅ **a11y-проход: focus-trap+Esc модалок + reading-Esc + radiogroup + FileTree-кламп** — **PR #249** (B10). TRAP_OVERLAYS — отсеяно (false-positive).
15. ✅ **БД-устойчивость: миграции-потолок, write_actor/read_pool catch_unwind** — **PR #241** (B2).
16. ✅ **Остаток minor: contradictions-honesty (#242 B3) · graph self-loop (#243 B4) · parser/scan/recurring (#245 B5) · news lossy+injection (#244 B6) · frontend-honesty data:image/ASCII-tag/relTime (#246 B7) · perf outline/drag/cache (#250 B11) · HN-фильтр (#253 B14).** Остаток (orphan-GC, reconcile-orphans, mtime-shortcut, should_generate, candidate_pairs, offline-banner, web-save, saveRemote) — отложен в `docs/BACKLOG.md` с обоснованием.

---

## Итог (2026-06-15)

**CRITICAL 1/1 закрыт · MAJOR 17/17 (15 закрыто PR, 2 отсеяно как false-positive) · MINOR/NIT — автоном-safe закрыты батчами B1–B14 (PR #240–253) + #231–238.** Отложено в BACKLOG с обоснованием: orphan-history-GC, reconcile-orphans (риск usearch v2 API), should_generate/candidate_pairs (нужна схема/миграция), NewsView-offline-banner (нужен egress-event), web-save-swallow + saveRemote (honesty-хвосты). Отклонено как перф-регрессия: mtime-shortcut. Отсеяно (false-positive): get_backlinks-occurrences, atom-date-concat, hotkey-ghost, graphview-rerender (уже #148), trap-overlays-stack.
