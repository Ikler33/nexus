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

- [ ] **delete/rename: гард служебных путей обходится через `..`** — живой `nexus.db`/`.nexus` можно отправить в корзину. `commands/vault.rs:717,751` + `vault/mod.rs:71-74,158-179`. Fix: проверять служебные пути ПОСЛЕ канонизации (компонентно, как `watcher::is_ignored`); строковый `starts_with` ещё и Windows-слаб.
- [ ] **rename/move не переносит `.nexus/history/<rel>/`** — история версий недоступна (CURATE-2 ломает SAFE-5/6). `commands/vault.rs:745-797`. Fix: после `fs::rename` переносить каталог истории (top-level rename для папки).
- [ ] **Молчаливое усечение векторов** при несовпадении длины ответа эмбеддера (zip-truncation). `indexer/mod.rs:358`, `ai/embedder.rs:130-142`, `rag.rs:191`. Fix: `Err` при `data.len() != inputs.len()`; инвариант `vectors.len()==chunk_ids.len()` перед записью.
- [ ] **get_backlinks дубли** — каждое повторное `[[упоминание]]` = отдельная строка (раздут счётчик). `graph/mod.rs:53-58`. Fix: `GROUP BY l.source_id`.
- [ ] **graph_rank неограниченные IN** → `too many SQL variables` / краш RAG-чата на супер-хабе. `search/mod.rs:405-455`. Fix: `graph::collect_in_chunks` (вынести в общий util) для обоих IN.
- [ ] **Вотчдог-таймаут оставляет джобу в running навсегда** → «Генерирую…» залипает. `scheduler/mod.rs:681-690,169-180`. Fix: `requeue_running` в ветке таймаута (или lease/claimed_at).
- [ ] **git force-checkout на грязном дереве** → молчаливая потеря незакоммиченных правок. `git/mod.rs:406-416` ← `commands/git.rs:131`. Fix: `status()`-guard перед checkout (или commit-then-pull-push).
- [ ] **plugin vault.writeFile не запрещает `.nexus/`** — плагин с `vault:write['**']` перезаписывает секреты/БД/код плагинов. `commands/plugin.rs:221-251`, `permission.rs:144-187`. Fix: отклонять `is_ignored`-пути в `check_path` независимо от glob-scope.
- [ ] **⌘L toggleTask портит нумерованные таски** `1. [ ] foo` → `- [ ] 1. [ ] foo`. `lib/editor/format.ts:63-72`. Fix: выровнять `transformTaskLine` с `TASK_LINE_RE`; тест на ordered-tasks.
- [ ] **dailyNote/quickThought перезаписывают файл при ЛЮБОЙ ошибке чтения** (try read / catch write). `components/home/HomeView.tsx:155-173`. Fix: проверять существование через `fileHash` (null только когда файла нет), как `lib/daily.ts:38`.
- [ ] **Регенерация удаляет обмен из БД ДО переспроса** — при сбое генерации обмен потерян. `stores/chat.ts:329-342`. Fix: `deleteLastExchange` в success-ветке (или восстановить при ошибке).
- [ ] **Quick-capture ⌘⇧N пишет Inbox.md мимо открытого грязного буфера** — мысль теряется при «Оставить мои». `lib/daily.ts:60-69`. Fix: буфер-aware `appendCapture` (как `inbox/actions.ts`).
- [ ] **Избранное не переживает rename/delete** — звёзды осиротевают; `starred.rename` мёртвый код. `stores/starred.ts:49-54`, `vault.ts:115-158`. Fix: звать `rename`/`dropStarsUnder` из vault-курации.
- [ ] **reloadWidget без try/catch** — спиннер «генерирую…» залипает при ошибке refetch. `stores/home.ts:103-117`. Fix: try/catch со снятием `generating` (как `refreshWidget`).
- [ ] **plugin vault.writeFile неатомарен** (`tokio::fs::write`) — окно повреждения .md. `commands/plugin.rs:247`. Fix: `vault::atomic_write` в `spawn_blocking`.
- [ ] **Конфиги egress-consent неатомарны** (egress/news/websearch/local.json, truncate-then-write). `net/persist.rs:60`, `news/config.rs:78`, `websearch/config.rs:73`, `commands/settings.rs:148`. Fix: общий атомарный JSON-врайтер.
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
1. **SSRF-кластер plugin-egress + is_private_host** (CRITICAL) — первым, эксфильтрация секретов/LAN.
2. **delete/rename path-traversal через `..`** (major) — security-граница ФС.
3. **`.nexus` писабелен/читаем плагином** (major) — вторая половина plugin-sandbox (один проход с #1/#2).
4. **Атомарность конфигов + plugin/news/export/vault.writeFile** (major+minor) — общий atomic-writer.
5. **rename переносит `.nexus/history` + GC истории/корзины** (major+minor) — зависит от #2.
6. **Потери данных в UI: dailyNote/regenerate/quick-capture** (3×major) — высокий impact / низкая цена.
7. **git force-checkout на грязном дереве** (major).
8. **zip-усечение векторов + scan silent-drop + reloadWidget залип** (major) — целостность RAG + честность.
9. **graph_rank unbounded IN + chunk_file_meta** — общий `collect_in_chunks`.
10. **toggleTask нумерованные + get_backlinks дубли** (major) — видимая порча/враньё.
11. **Планировщик: watchdog requeue + recurring re-arm + contradictions seed** (major+minor).
12. **Cross-feature синхронизация: starred/recents/navHistory/suggest при курации/смене-vault** (major+minor).
13. **Фронт-гонки эпох-гард: loadSession/news/onEvent + honesty: web-save/SyncPanel/home** (minor).
14. **a11y-проход: focus-trap+Esc модалок + reading-Esc + TRAP_OVERLAYS_CLOSED + radiogroup + FileTree** (minor).
15. **БД-устойчивость: миграции-потолок, write_actor/read_pool catch_unwind** (minor).
16. **Остаток minor + nit** — фоновая чистка по подсистемам.
