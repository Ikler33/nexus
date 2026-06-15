# Дотошный код-аудит Nexus — 2026-06-15

> Мультиагентный аудит (21 финдер по подсистемам + cross-cutting линзы → независимая верификация
> КАЖДОГО замечания по коду → дедуп → ранжирование). 132 агента, 102 подтверждённых замечания →
> **88 уникальных после дедупа: 1 critical · 17 major · 33 minor · 38 nit.** Контроль ложных
> срабатываний: каждое замечание подтверждено скептиком против реального кода (отклонённые отброшены).
>
> Очередь исправления — §«Порядок» внизу. Статусы: `[ ]` открыто · `[x]` закрыто (PR#) · `[-]` отложено.

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
- [ ] **get_backlinks дубли** — каждое повторное `[[упоминание]]` = отдельная строка (раздут счётчик). `graph/mod.rs:53-58`. Fix: `GROUP BY l.source_id`.
- [x] **graph_rank неограниченные IN** — **ЗАКРЫТО (PR #236).** Оба `IN` через `graph::collect_in_chunks` (стал `pub(crate)`, чанк ≤900): BFS — две одиночные стороны рёбер (`source`/`target`), chunks-query чанкуется. Тест super-hub N=1000/hops=2.
- [x] **Вотчдог-таймаут оставляет джобу в running навсегда** — **ЗАКРЫТО (PR #238).** `requeue_running` (running→pending) в Err-ветке таймаута тика — как crash-recovery на старте воркера.
- [ ] **git force-checkout на грязном дереве** → молчаливая потеря незакоммиченных правок. `git/mod.rs:406-416` ← `commands/git.rs:131`. Fix: `status()`-guard перед checkout (или commit-then-pull-push).
- [x] **plugin vault.writeFile не запрещает `.nexus/`** — **ЗАКРЫТО (PR #232).** `is_escaping` блокирует сегменты `.nexus`/`.git` (case-insensitive) ДО scope — все vault-методы плагина (read/write/list) через `check_path`. Исходно: плагин с `vault:write['**']` перезаписывал секреты/БД/код плагинов.
- [x] **⌘L toggleTask портит нумерованные таски** — **ЗАКРЫТО (PR #237).** `transformTaskLine` зеркалит `TASK_LINE_RE` по набору маркеров (`[-*+]`/`\d+[.)]`) и СОХРАНЯЕТ нумерованный маркер при реконструкции (`1. [ ] x`↔`1. [x] x`); буллеты `-*+` по-прежнему нормализуются в `- `. +2 теста.
- [x] **dailyNote/quickThought перезаписывают файл при ЛЮБОЙ ошибке чтения** — **ЗАКРЫТО (PR #233).** Переведены на `openOrCreateDaily`/`openOrCreateInbox` с `fileHash`-проверкой существования (null только при отсутствии файла); дневник унифицирован с ⌘⇧D (`Journal/`). `components/home/HomeView.tsx`, `lib/daily.ts`.
- [-] **Регенерация удаляет обмен из БД ДО переспроса** — **НЕ БАГ (отсеяно адверсариально, аудит-ревью PR #233).** `stores/chat.ts:329-342`: ждёт персист прошлого обмена, ре-чекает дрейф ленты, чистит БД (`deleteLastExchange`) только для персистнутого **не-ошибочного** обмена (`sid != null`), текст вопроса сохраняется в `question` и переспрашивается. Потери нет.
- [x] **Quick-capture ⌘⇧N пишет Inbox.md мимо открытого грязного буфера** — **ЗАКРЫТО (PR #233).** `appendCapture` теперь буфер-aware: открыт Inbox → `updateBufferDoc`, иначе атомарный диск. `lib/daily.ts`.
- [ ] **Избранное не переживает rename/delete** — звёзды осиротевают; `starred.rename` мёртвый код. `stores/starred.ts:49-54`, `vault.ts:115-158`. Fix: звать `rename`/`dropStarsUnder` из vault-курации.
- [x] **reloadWidget без try/catch** — **ЗАКРЫТО (PR #233).** Обёрнут в try/catch со снятием `generating[key]` при ошибке (как `refreshWidget`). `stores/home.ts`.
- [x] **plugin vault.writeFile неатомарен** — **ЗАКРЫТО (PR #234).** `vault::atomic_write_io` в `spawn_blocking` (окно повреждения .md устранено).
- [x] **Конфиги egress-consent неатомарны** — **ЗАКРЫТО (PR #234).** Единый `vault::atomic_write_io` (tmp→fsync→rename) во всех 4 конфиг-врайтерах + news/chat-экспорт (7 сайтов). Тест no-leftover-tmp.
- [ ] **Web-поиск настройки молча проглатывают ошибку записи** — пользователь думает, что сохранил. `components/settings/SettingsView.tsx:553-560`. Fix: `.catch` + error-состояние (как `EgressBlock.apply`).

---

## MINOR (33) — кратко (title · location · fix)

- [ ] traversal истории: `list_versions/read_version` не валидируют `rel` · `commands/vault.rs:801-821` · `resolve_vault_path_for_write` на rel.
- [ ] осиротевшая история/корзина не GC при delete (рост диска) · `commands/vault.rs:713-738` · best-effort удаление history + ретенция корзины.
- [ ] TOCTOU снапшота истории (две записи на одну заметку) · `vault/history.rs:93-98` · `create_new` O_EXCL.
- [ ] reconcile_vectors не чистит orphan-векторы · `indexer/rag.rs:144-209` · reverse-проход / AUTOINCREMENT id.
- [ ] mtime-шорткат пропускает правку той же секунды при равном размере · `indexer/mod.rs:140` · наносекунды/хеш-сверка.
- [ ] несбалансированный `[[` матчит через документ · `parser/mod.rs:160-178` · ограничить поиск `]]` строкой.
- [ ] Atom: второй date-элемент конкатенируется → published_at=0 · `news/parse.rs:96-159` · присваивать, не аппендить.
- [ ] HN: двойная keyword-фильтрация отбрасывает совпавшие без story_text · `news/run.rs:65-208` · не фильтровать Algolia повторно.
- [ ] Дайджест: контент заметок в LLM без injection-маркеров · `digest/mod.rs:66-88` · обернуть в `injection_marker`.
- [ ] фид не-UTF8 → жёсткий отказ · `news/fetch.rs:137-146` · фолбэк по charset/lossy.
- [ ] self-backlink `[[A]]` в A.md + self-loop ребро · `graph/mod.rs:53-58,333-424` · `AND source_id != target_id`.
- [ ] get_full_graph: степень завышена дублями/self · `graph/mod.rs:379-392` · `COUNT(DISTINCT)` без self.
- [ ] contradictions: пустой результат → `should_generate` вечно-true · `contradictions/mod.rs:304-316` · хранить `last_run` отдельно.
- [ ] contradictions seed без `has_ready_job` → дубль джобы · `commands/vault.rs:390-399` · `reschedule_if_absent`.
- [ ] relation_reasons: кэш-хит игнорит пустое объяснение → LLM пере-вызов · `relation_reasons/mod.rs:132-149` · флаг «генерили пусто».
- [ ] hash_snippet на `DefaultHasher` (нестабилен по версиям Rust) · `contradictions/mod.rs:188` · blake3.
- [ ] contradictions::list отдаёт пары удалённых заметок · `contradictions/mod.rs:264-283` · JOIN `is_deleted=0`.
- [ ] recurring dead-джоба не переназначается · `scheduler/mod.rs:481-491` · re-arm при терминальной неудаче.
- [ ] write_actor умирает при первой панике (докстринг обещает rollback) · `db/write_actor.rs:30-70` · `catch_unwind` (debug-only, release=abort).
- [ ] read_pool теряет conn при панике замыкания · `db/read_pool.rs:43-71` · RAII-guard возврата (debug-only).
- [ ] миграции: нет потолка версии (downgrade молча работает) · `db/migrations.rs:138-161` · `Err` при `current > latest`.
- [ ] suggest `dismissed` кэш не чистится при смене vault · `stores/suggest.ts:22-50` · `clearDismissed()` в `openVault`.
- [ ] home `error`/`loading` не читаются HomeView (мёртвые поля) · `stores/home.ts` + `HomeView.tsx:86-99` · подключить баннер/скелет.
- [ ] news.load без эпох-гарда · `stores/news.ts:58-173` · epoch-guard.
- [ ] loadSession проверяет streaming только на входе · `stores/chat.ts:387-423` · перепроверка после await.
- [ ] inline `data:`-картинка вычищается urlTransform · `MarkdownPreview.tsx:26-72` · разрешить `data:` для img.
- [ ] OutlineBar пере-извлекает заголовки на каждый символ · `OutlineBar.tsx:16` · дебаунс/ленивость.
- [ ] чат onEvent без эпох-гарда (поздние token после stop) · `stores/chat.ts:234-291` · streamId/epoch.
- [ ] radiogroup режима без стрелок/roving tabindex · `ChatView.tsx:201-216` · roving tabindex + onKeyDown.
- [ ] drag-ресайз панели не чистит листенеры при unmount · `AiPanel.tsx:202-220` · cleanup/AbortController.
- [ ] disclosure-переполнение чистит ВСЕ раскрытия · `ChatView.tsx:435-440` · LRU вместо clear.
- [ ] AiPanel/bucketOf без тестов · `AiPanel.tsx:60-170` · AiPanel.test.tsx.
- [ ] модалки Digest/Contradictions/Settings без focus-trap/Esc · `*Panel.tsx` · `useFocusTrap`.
- [ ] reading-Esc закрывает чтение вместо модалки · `App.tsx:176-196` · добавить флаги модалок.
- [ ] FileTree active не клампится при сжатии → битый aria-activedescendant · `FileTree.tsx:69,152` · кламп.
- [ ] NewsView offline-баннер fire-once · `NewsView.tsx:93-101` · перечитывать по событию/фокусу.
- [ ] ремап хоткея не снимает старый дефолт (ghost binding) · `lib/commands.ts:160-216` · исключать оверрайднутые из скана.
- [ ] remarkNexus подсвечивает кириллический #тег, бэкенд не создаёт · `lib/markdown/remarkNexus.ts:12` vs `parser/mod.rs:184` · свести к одному источнику.
- [ ] relTime показывает «1 минуту назад» для будущего · `lib/time.ts:3-11` · signed diff.
- [ ] candidate_pairs N+1 по read-пулу · `contradictions/mod.rs:87-114` · батч-выборка.
- [ ] GraphView перерисовывает весь SVG на тик · `GraphView.tsx:393-743` · мемо-подкомпоненты / freeze.
- [ ] кэши без cap (startingQuestions/relations) · `startingQuestionsCache.ts:11` · LRU/cap.
- [ ] news/export пишутся неатомарно (новые файлы → minor) · `commands/news.rs:466`, `chat_sessions.rs:178` · `atomic_write`.
- [ ] reset() чистит recents только в памяти, не localStorage · `stores/workspace.ts:534-545` · `saveRecents([])`.
- [ ] ConflictResolver без Esc + вне reading-Esc списка · `ConflictResolver.tsx:108-127` · Esc + флаг.
- [ ] capture/templates/versions вне TRAP_OVERLAYS_CLOSED (стек 2 модалок) · `stores/ui.ts:126-199` · добавить в группу.
- [ ] moveTab не ремапит navHistory groupId · `stores/workspace.ts:286-307` · ремап.
- [ ] SyncPanel commit/saveRemote молча глотают ошибку · `SyncPanel.tsx:58-88` · показать ошибку.
- [ ] упавшие при скане файлы молча выпадают, прогресс 100% · `indexer/mod.rs:543-558` · счётчик failed в прогресс.

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
7. **git force-checkout на грязном дереве** (major). ← следующий.
8. ✅ **zip-усечение векторов** (major) — **PR #235** (`CountMismatch`+инвариант). (scan silent-drop — в #16; reloadWidget — в #233.)
9. ✅ **graph_rank unbounded IN** — **PR #236** (`collect_in_chunks` pub(crate)). chunk_file_meta — уже чанкуется.
10. ✅ **toggleTask нумерованные** (major) — **PR #237.** get_backlinks дубли — в #16 (нужна UX-сверка: список occurrences vs счётчик).
11. ✅ **Планировщик: watchdog requeue** (major) — **PR #238.** recurring re-arm/contradictions seed — в #16.
12. **Cross-feature синхронизация: starred/recents/navHistory/suggest при курации/смене-vault** (major+minor). ← осталось.
13. **Фронт-гонки эпох-гард: loadSession/news/onEvent + honesty: web-save/SyncPanel/home** (minor).
14. **a11y-проход: focus-trap+Esc модалок + reading-Esc + TRAP_OVERLAYS_CLOSED + radiogroup + FileTree** (minor).
15. **БД-устойчивость: миграции-потолок, write_actor/read_pool catch_unwind** (minor).
16. **Остаток minor + nit** — фоновая чистка по подсистемам.
