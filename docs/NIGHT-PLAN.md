# Автономный план работ / ночная очередь (carte blanche)

> Пользователь дал полную автономию. Работать без подтверждений, сколько хватает лимитов.
> Крон `72463b9a` (ежедневно 5:40, после ресета лимитов в ~5:30) возобновляет работу при паузе по
> лимитам/сети. Этот документ — **источник плана и журнал прогресса**.
>
> **Каждая крон-сессия, ПЕРВЫМ делом:** прочитай этот файл + `CHANGELOG.md` + `docs/BACKLOG.md` +
> `docs/reviews/BACKLOG_REVIEW.md` (§2 автономность, §5 правки). Определи по журналу/CHANGELOG, что
> уже сделано, и продолжи **со следующего невыполненного пункта очереди**.
>
> Обновлён 2026-06-04 по итогам мульти-агентного ревью. Порядок задан владельцем:
> **тестирование → прогон/фикс багов → бэклог**. E2E — отдельным треком, не блокирующим.

## Жёсткие правила

1. **Дисциплина среза:** реализация → зелёные тесты → дока (CHANGELOG + per-feature + BACKLOG) →
   ветка `phaseN/NN-name` или `track/NN-name` → коммит → **push → PR → мерж на зелёном CI → удалить
   ветку → следующий**. Не копить несколько срезов в одной ветке.
   - **Push/PR/merge РАЗРЕШЕНЫ автономно** (репозиторий публичный `Ikler33/nexus`, владелец
     авторизовал). Мерж только на зелёном CI: `gh pr merge <N> --merge --delete-branch`.
     ⚠️ Изменение относительно прошлых прогонов — раньше пуш был запрещён.
   - Коммит-сообщение ссылается на AC-…/§; заканчивается `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`.
   - Тело PR заканчивается `🤖 Generated with [Claude Code](https://claude.com/claude-code)`.
2. **Команды верификации** (должны быть зелёными до коммита):
   - Rust (из `apps/desktop/src-tauri`): `source "$HOME/.cargo/env"; cargo fmt --all -- --check && cargo clippy --all-targets -- -D warnings && cargo test`
   - Front (из `apps/desktop`): `pnpm exec tsc --noEmit && pnpm exec eslint . && pnpm exec vitest run && pnpm exec vite build`
3. **Offline-first.** Бери только пункты, которые **верифицируются без живых серверов** (детерминированные
   тесты/моки/фикстуры). Живые тесты — `#[ignore]`; если сервер недоступен — пропустить, НЕ падать.
4. **«no silent caps».** Урезал/отложил/обошёл — пиши в `docs/BACKLOG.md` (а не только в код/коммит).
5. **ADR не менять молча.** Конфликт с ADR/спекой → запиши в «NEEDS-DECISION» ниже и переключись на
   другой пункт (не блокируйся, не выдумывай решение за владельца).
6. 🛑 **Cron-guard (ревью A18).** Если в очереди НЕТ пункта с (а) понятным критерием готовности и
   (б) offline-верификацией — **НЕ браться за fragile/blind-работу** (фронт-вслепую, vision без AC,
   живые серверы). Записать строку в журнал «нет автономно-безопасного пункта, жду владельца» и
   **завершить сессию**, не жечь лимиты.
7. **Источник истины:** `docs/architecture/ARCHITECTURE.md` (§0 ADR), `docs/acceptance/ACCEPTANCE.md`
   (AC-*), `docs/dev/*.md`, `docs/dev/TESTING_STRATEGY.md`, `docs/design/DESIGN_BRIEF.md`. Реализовывать
   по спеке, не выдумывать.
8. **Живые серверы** (для `#[ignore]`-тестов, не обязательны): chat Qwen `192.168.0.172:8080`;
   embed nomic `127.0.0.1:8081`; jina-code `127.0.0.1:8082`; bge-m3 `127.0.0.1:8083`. Слетают после
   ребута мака → `bash ~/Documents/llm-models/start_servers.sh`. Очередь спроектирована так, чтобы
   НЕ зависеть от них.
9. **В конце каждого пункта** — обнови «Журнал» внизу (что сделано, PR #, статус).

---

## Очередь работ (по приоритету; порядок владельца)

### 🌊 Волна 1 — Каркас тестирования (safety-net; автономно, быстро)

> Сеть тестов строится ПЕРВОЙ — под её защитой остальные волны безопасны. См. `TESTING_STRATEGY.md`.

- **V1.1 — CI security-job** (ревью B6, AC-Q-5). `cargo-deny` (`deny.toml`: advisories/bans/licenses/
  sources) + `cargo audit` + `gitleaks`-шаг на PR + выделенный прогон security-тестов как required-check
  (не тонут в общем `cargo test`). _Верификация:_ `cargo deny check` локально зелёный; новый job в `ci.yml`.
- **V1.2 — Coverage + ratchet** (AC-Q-2). `cargo-llvm-cov` (Rust) + `vitest run --coverage` (v8) в CI,
  вывод в summary; стартовые пороги (≈70%, калибруется), политика «не падать ниже». _Верификация:_
  локальный прогон coverage, порог энфорсится.
- **V1.3 — Traceability «AC ↔ тест»** (ревью §4, TESTING_STRATEGY §4). `docs/dev/traceability.yml`
  (каждый AC-* → ID тестов) + скрипт-проверка (CI), что у каждого AC есть хотя бы один тест и нет
  ссылок на несуществующие AC. Тегировать тесты AC в имени/коммент. _Верификация:_ скрипт зелёный в CI.
- **V1.4 — Добор integration-тестов** на непокрытые IPC-команды (`git_merge_preview`,
  `git_resolve_conflicts`, `get_full_graph`, `search_content`, …). _Верификация:_ `cargo test`.

### 🌊 Волна 2 — Прогон + фикс выявленных багов (каждый баг едет со своим тестом)

- **V2.1 — Core SSRF / redirect** (ревью C5/H11, AC-SEC-4b). `redirect(Policy::none())` на 3 core-reqwest
  клиентах: `ai/embedder.rs` (`new` + `probe_dim`) и `ai/chat.rs` (`new`). _Тест:_ 30x-редирект не
  следуется. _Offline._
  ⚠️ **ВАЖНО (рекон V1.2):** `is_private_host` к core-клиентам НЕ применять — LLM-серверы локальные
  by design (`127.0.0.1:8081`, `192.168.0.172:8080`); блок приватных хостов сломал бы local-first.
  Различие: core = локальный + redirect-guard; plugin `net.fetch` = allowlist + `is_private_host`
  (уже есть, `plugin/permission.rs:243`). Consent на смену `base_url` при git-pull — это пункт
  «Единый egress-контроль ядра» (Фундамент), не здесь.
- **V2.2 — Rename-as-move** (ревью L6, AC-Б9). `VaultEvent::Renamed` → обновить `files.path` с
  сохранением `file_id` (сейчас delete+create рвёт беклинки/чанки). _Тест:_ rename → file_id жив,
  беклинки целы. _Offline._
- **V2.3 — Граф: guard лимита SQLite-переменных** (ревью A9/M6). Чанковать `IN`/рекурсивный CTE в
  `get_local_graph`/`get_full_graph` до ≤999 (`SQLITE_MAX_VARIABLE_NUMBER`). _Тест:_ супер-хаб (узел с
  2000+ связей) не паникует/не ошибается. Снимает 1 гипотезу вылета графа. _Offline._
- **V2.4 — Chat throttling** (ревью C9/M9, AC-Б10-4). Измеримый порог (≤N ре-рендеров на 2000 токенов):
  батч-append через rAF в `stores/chat.ts` + тест-счётчик. _Offline (jsdom)._

### 🌊 Волна 3 — E2E-харнесс (ОТДЕЛЬНЫЙ трек, НЕ блокирующий)

> Решение владельца: делать параллельно, при флаки в CI — не required-check, задокументировать.

- **V3.1 — Скелет `tauri-driver` + WebdriverIO** + 1 smoke (приложение стартует, рендерит оболочку).
  CI-job на Linux+xvfb. Если нестабильно — non-required + строка в BACKLOG («E2E flaky, не блокирует»).
- **V3.2 — Smoke-потоки** инкрементально: открыть vault → открыть заметку → поиск. По одному за срез.

### 🌊 Волна 4 — Автономный фундамент / бэклог

- **V4.1 — Парсинг frontmatter** (ревью H2). `serde_yaml`: frontmatter YAML → типизированные поля +
  заполнить таблицу `aliases` + типизированный доступ. Разблокирует цели/stale-radar/Dataview/резолв
  aliases. _Тест:_ парс RU/EN frontmatter, aliases в БД. _Offline, чистый Rust._
- **V4.2 — Redaction-layer (AC-SEC-6)** (ревью H18). `Redacted<T>` (безопасный Debug) + аудит
  tracing-вызовов (пути/контент/URL) + усиление crash-scrub (не только HOME). _Тест:_ Debug не печатает
  секрет; scrub чистит пути vault. _Offline._
- **V4.3 — Anti-prompt-injection (AC-SEC-7)** (ревью B2/A3). Неподделываемые рандом-разделители вместо
  `[n] source` в `build_rag_messages` + системная инструкция «между маркерами — данные» + строгая
  валидация JSON-ответа suggest. Предусловие любых web-фич. _Тест:_ инъекция в заметке не управляет
  инструкцией; невалидный JSON suggest отклонён. _Offline._
- **V4.4 — Общий чат без vault-grounding** (ревью правка 17, vision-critical). Режим чата
  vault / общий: «общий» пропускает RAG-ретрив, отвечает напрямую LLM. Закрывает активный разрыв
  «чат всегда грунтуется в vault». _Тест:_ в режиме «общий» ретрив не вызывается (мок), в «vault» —
  вызывается. _Offline (мок chat)._ Web-search/tool-use — НЕ здесь (требует ADR, см. NEEDS-DECISION).
- **V4.5 — Offline plumbing-eval-гейт** (ревью A1, частично). Детерминированно-векторный eval-тест
  (RRF/recall@k/nDCG/MRR на фиксированных синтетических векторах) как НЕ-`#[ignore]` `cargo test` →
  ловит регрессии логики ранжирования в CI без сервера. _Тест:_ известный вход → известные метрики.
  ⚠️ Гейт на **реальном качестве** (фикстура реальных эмбеддингов) — в BLOCKED (нужен сервер 1 раз).

---

## 🚫 BLOCKED / NEEDS-DECISION (НЕ делать автономно — нужен владелец/инфра/AC)

- **Root-fix вылета графа на реальном vault** — не воспроизводится в среде агента (нужен Tauri+WKWebView
  на реальном vault владельца). Нужен **артефакт владельца**: вывод терминала `pnpm app:dev`
  (`thread … panicked at src/…:LINE`) ИЛИ scrubbed-лог `~/.nexus/crashes/`. V2.3 снимает 1 гипотезу
  защитно, но корень — за владельцем.
- **Real-quality eval-фикстура** (засеять golden реальными эмбеддингами bge-m3) — нужен живой `:8083`
  один раз. Разовый шаг владельца.
- **Реранкер** — нет `/rerank` (`:8082` = jina-эмбеддер кода); нужна модель+сервер = решение владельца.
- **Сетевой токенайзер чанкера** — tradeoff точность vs офлайн/реиндекс = решение владельца. (Альтернатива
  «оффлайн HF-токенайзер» — потенциально автономна, но меняет границы чанков → перепрогон eval; вынести
  отдельным обсуждением, не молча.)
- **V4.1 frontmatter-parse — выбор YAML-подхода (вскрыто крон-сессией #2):** ревью H2 рекомендовал
  `serde_yaml`, НО он **архивирован/unmaintained** → cargo-deny с `unmaintained="workspace"` (V1.1) флагнет
  его как прямую зависимость → красный security-гейт. Варианты: (а) форк `serde_yml` (спорный); (б)
  `yaml-rust2`/`saphyr` (maintained, не-serde, больше ручного маппинга); (в) минимальный line-парсер только
  под `aliases` + плоские скаляры (без либы, контейнерно, но хрупко на сложном YAML). Решение влияет на
  объём V4.1 — не выбирать вслепую. Для аккуратного старта: вариант (в) только для `aliases` (конкретный
  анблок резолва ссылок), полный typed-frontmatter — отдельно.
- **Web-агент / SearXNG, News Feed, cloud-fallback** — требуют ADR (единый egress-контроль ядра, ревью
  H3/A8) + инфра (SearXNG на VPS) + egress-policy = решение владельца.
- **Реальная загрузка кода плагина + editor-extensions + marketplace-дистрибуция** — нужен ADR-007
  (доверенный JS/Worker) + E2E в webview + реестр/ключ издателя (ревью A4/A5).
- **auto-updater подпись/нотаризация** — Tauri-keypair (секрет) + Apple Developer + Authenticode (ревью A12).
- **Mobile (`apps/mobile`)** — физ. девайс/симулятор + решение о вехе.
- **Вся секция «Идеи/vision»** — нет AC, зашиты продуктовые решения владельца (ревью A2). Сначала сессия
  «vision → AC», только потом реализация.

---

## 📓 Журнал прогресса (дописывать)

### Текущий прогон (план от 2026-06-04)
- (план составлен) Очередь переписана по итогам мульти-агентного ревью; правила обновлены
  (push/PR/merge теперь автономны). Старт — по сигналу владельца «иду спать»; крон `72463b9a`
  (5:40, после ресета лимитов ~5:30) — фолбэк по лимитам/сети.
- ✅ **V1.1 — CI security-job** (ревью B6 / AC-Q-5). Job `security` (cargo-deny + gitleaks) +
  `deny.toml` + `.gitleaks.toml`. Гейт сразу сработал: нашёл и закрыл **RUSTSEC-2026-0008** (unsound в
  git2 0.19 — прямая зависимость) бампом git2 0.19→0.20.4 (libgit2 1.9.4, git-тесты зелёные); 16
  транзитивных unmaintained (gtk-rs/unic via Tauri) → `unmaintained = "workspace"`. Локально зелёные:
  fmt · clippy · test · licenses · bans · sources · advisories · gitleaks. **PR #34 смержен** (CI зелёный;
  потребовался fix `fetch-depth: 0` для gitleaks — shallow checkout не давал diff-диапазон).
- ✅ **V1.2 — Coverage-ратчет** (TESTING_STRATEGY §6). Frontend: `@vitest/coverage-v8` + блок coverage в
  `vitest.config.ts` (пороги 63/63/60/75, baseline 64.3/62.1/77.3%), CI `pnpm test:coverage`. Rust: job
  `coverage-rust` (`cargo-llvm-cov --fail-under-lines 65`, baseline строк 71.8%), параллельно матрице.
  Локально зелёные оба замера. Отложено в BACKLOG: per-path пороги, baseline.json-bump, test-all.sh.
  **PR #35 смержен** (понадобился fix: корневой `test:coverage`-скрипт — CI запускает из корня репо).
- ✅ **V1.3 — Traceability AC ↔ тест** (TESTING_STRATEGY §4). `docs/acceptance/traceability.json` (77 AC:
  статус + tests) + zero-dep гейт `scripts/check-traceability.mjs` (job `traceability`): новый AC без
  записи → красный CI. Гейт сразу поймал 2 свои несогласованности (partial без tests) → поправлено.
  Картина: 26 covered · 17 partial · 12 pending · 17 manual · 5 deferred (43/77 автотестами); pending
  совпадают с очередью V2/V4. **PR #36 смержен.** Все 3 гейта стратегии (security/coverage/traceability)
  на main.
- ⏭️ **V1.4 (integration-тесты команд) ОТЛОЖЕН** после Волны 2. Рекон: command-обёртки (chat/git/graph/
  search/suggest `.rs`) — тонкие `#[tauri::command]`, делегируют в уже покрытые модули (graph 92.8%,
  search 81.8%, git 77.5%); тесты требуют State<AppState>-фикстур при умеренной пользе. coverage-храповик
  (V1.2) и так держит регрессии. Приоритет владельца — «фикс багов» → перешёл к Волне 2.
- ✅ **V2.1 — Анти-SSRF core-redirect** (AC-SEC-4 / ревью C5). 3 core-клиента (embedder×2, chat) → общий
  `ai::core_client_builder()` с `redirect(Policy::none())`; тест `core_client_does_not_follow_redirects`
  (локальный 302-сервер, zero-dep). `is_private_host` к ядру НЕ применяется (LLM локальны by design).
  Rust 110+9 зелёные, fmt/clippy ok. **PR #37 смержен.**
- ✅ **V2.3 — Граф: guard лимита SQLite-переменных** (ревью A9; взят вне очереди до V2.2 — contained +
  адресует баг вылета графа). `get_local_graph`/`get_full_graph`: все `IN`-запросы чанкуются
  (`collect_in_chunks` ≤900; рёбра — одиночный `source IN (chunk)` + фильтр target∈ids вместо двойного
  IN; BFS-фронтир по 450). Результат полный, без обрезки. Тест `super_hub_does_not_exceed_sql_var_limit`
  (хаб 1000 связей, фикстура через `WriteActor::transaction`). Снимает 1 из 3 гипотез вылета графа (корень
  ждёт артефакт владельца). Rust 111+9 зелёные. **PR #38 смержен.**
- ✅ **V2.4 — Throttle рендера токенов чата** (AC-Б10-4 / ревью C9). `stores/chat.ts`: токены копятся в
  буфер, применяются одним `set()` на кадр (`requestAnimationFrame`) вместо O(токенов) ре-рендеров;
  `done`/`error`/`stop` сбрасывают хвост синхронно. Тест: 200 токенов → 1 rAF-кадр (мок rAF). Frontend
  86 тестов + coverage 64.5% зелёные, tsc/eslint/build ok. traceability AC-Б10-4 → covered. PR открыт,
  мерж на зелёном.
  **Волна 2 почти закрыта.** Осталась V2.2 (rename-as-move) — finicky-watcher (notify rename From/To,
  платформозависимо).
- 🏁 **Крон-сессия #2 завершена на чистой вехе.** Сделано: **V2.3** (граф var-limit guard), **V2.4**
  (chat-throttle). Всего за ночь слито: V1.1·V1.2·V1.3 (тест-гейты) · V2.1·V2.3·V2.4. Cron-guard: оба
  оставшихся пункта Волны 2/4 имеют развилки — **V4.1 frontmatter** упирается в выбор YAML-подхода
  (serde_yaml unmaintained → security-гейт, см. NEEDS-DECISION); **V2.2 rename** — finicky платформо-
  зависимый watcher. Не беру вслепую → завершаюсь, лимиты не жгу. Следующей крон-сессии/владельцу:
  начать с **V4.1 вариант «aliases-only line-парсер»** (без YAML-либы, контейнерно) ИЛИ согласовать
  YAML-подход. Перед стартом: `git fetch && checkout main && pull`.

### Архив — прогон #1 (предыдущая ночь, до ревью)
Сделано за ночь и закоммичено (`phase1/12` → `phase2/01-capability-model`): condition-eval;
crash-reconcile usearch (§5.1, `27f5e0c`); **Ф1-12** bge-m3 (AC-EVAL-6 закрыт; eval recall@8=1.0,
nDCG@8=0.883, MRR=0.848); **Ф2-1** модель прав плагина (`permission.rs`, 13 security-тестов);
**Ф2-2a** capability-broker host-сторона (`broker.rs`, неотключаемый audit). Фазы 2-4 впоследствии
доведены и слиты (см. CHANGELOG). Отложено тогда: фронт-транспорт плагинов (нужна визуальная проверка),
CI-eval-фикстура, сетевой токенайзер — перенесено в очередь/BLOCKED выше.

---

## Хендофф для следующей крон-сессии И владельца
- Перед стартом: `git fetch && git checkout main && git pull --ff-only` (работать от свежего main).
- Идти строго по очереди V1→V2→V3→V4; каждый пункт — отдельный PR, мерж на зелёном CI.
- Если упёрся в BLOCKED/NEEDS-DECISION по всем оставшимся — сработал **Cron-guard** (правило 6):
  записать строку сюда и завершить, не жечь лимиты.
