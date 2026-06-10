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
8. **Живые серверы** (для `#[ignore]`-тестов, не обязательны) — переехали на отдельный хост
   **`192.168.0.29`** (2026-06): chat **Gemma** `:8080` (`gemma-4-26B-A4B-it`, контекст 256K);
   RAG-эмбеддинги **bge-m3** `:8083` (dim **1024**, мультиязычные); nomic `:8081` (768, англ/запас);
   jina-code `:8082` (768). SSH `serv@192.168.0.29`. Очередь спроектирована так, чтобы НЕ зависеть от них.
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
- ~~**V4.1 frontmatter-parse — выбор YAML-подхода**~~ **✅ РЕШЕНО владельцем: вариант (в) — расширенный
  мини-парсер** (без YAML-либы; serde_yaml архивирован → security-гейт). `aliases` (V4.1) + плоские
  скаляры `progress/due/goal/evergreen/draft`… → таблица `frontmatter_fields`. Сложный вложенный YAML —
  fallback на сырой `frontmatter`. Типизация значений/query-API — по мере консьюмеров (BACKLOG).
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

### Утро — продолжение с владельцем
- ✅ **V4.1 — Frontmatter `aliases` + резолв `[[Алиас]]`** (выбор владельца: вариант aliases-only
  line-парсер, без YAML-либы). Парсер: 3 формы (инлайн/блок/скаляр). Индексатор: таблица `aliases`
  (OR REPLACE на UNIQUE), `resolve_target`/`resolve_all_dangling` + обратный резолв расширены на алиасы
  (forward+backward, путь приоритетнее). Rust 113+9 зелёные. Отложено: **полный typed-frontmatter** —
  NEEDS-DECISION по YAML-подходу (записано). **PR #40 смержен.**
- ✅ **«Вылет графа» РАЗГАДАН — НЕ краш** (диагностика с владельцем на реальном vault SA-Vault, 122
  файла). Лог Rust: паники нет, процесс один раз стартовал; перезагрузился только webview после
  `[vite] ✨ new dependencies optimized: graphology-layout-forceatlas2 … reloading`. Корень: dev-only
  ленивая до-оптимизация Vite граф-зависимостей → full-reload. Фикс: `optimizeDeps.include`
  (graphology/sigma/forceatlas2) в `vite.config.ts`. Прод-сборки не касалось. Баг в BACKLOG → закрыт.
  **PR #41 смержен.** Последний 🔴-баг снят.
- ✅ **Граф: интерактив по дизайну** (выбор владельца; новый дизайн `graph.jsx` из Hermes.zip). sigma.js →
  кастомный **SVG force-directed**: drag (соседи подтягиваются), hover-подсветка, активная нота
  пульс/ripple/кольцо, kin-кольца, «поток» по рёбрам, local(глубина)/full, счётчик, загрузка. Логика —
  `graph-sim.ts` (8 юнит-тестов); view `GraphView.tsx` — human-verify (исключён из coverage). Удалены
  sigma/graphology/forceatlas2 + worker + optimizeDeps. Frontend 90 тестов + coverage 67.95% зелёные,
  tsc/eslint/build ok. ⚠️ Симуляция main-thread (worker-layout заменён, AC-PERF-6) — узлы капнуты.
  Отложено: теги-цвета/фильтр (нужны теги на узлах из БД) + render-smoke. **PR #42 смержен.**
- 🔄 **Граф v2** (выбор владельца «полный v2 разом»; отзыв vs Obsidian: мешанина/резкость/градации +
  «можно ли переиспользовать Obsidian» → нет, закрыт; берём d3-force — та же открытая основа).
  Стопгап-тюнинг #43 закрыт (суперсиднут).
  - ✅ **v2a — физика на d3-force**: forceManyBody/Link/Center/Collide; drag через fx/fy (пин +
    сопротивление связанных). Рендер SVG + анимации сохранены. `graph-sim.ts` ужат до помощников
    (4 теста). Frontend 86 тестов + coverage 66.9% зелёные. **Ветка `track/11-graph-d3force`, владелец
    проверяет физику-feel перед мержем** (числа сил подстрою по отзыву).
  - ✅ **v2d — панель настроек физики ⚙️** (отзыв «настройки не регулируются» + слепой цикл подгонки
    сил → отдаём руль владельцу). Слайдеры Отталкивание/Длина связей/Притяжение к центру/Размер —
    применяются вживую (мутация сил через refs + alpha-restart, позиции сохраняются) + localStorage.
    Каноничный фикс разлёта: убран жёсткий link.strength → d3 авто-масштабирует рёбра к хабам;
    заряд по степени; forceX/Y-«гравитация» вместо forceCenter; pin не навсегда. **PR #44 смержен.**
  - ⏳ **v2e** дефолты-«сфера» (отзыв: на дефолтах размазано; решается слайдером, дефолт подобрать) ·
    **v2b** граф-во-вкладку «Граф» · **v2c** пан/зум-камера + авто-fit. Незаблокирующее (решение
    владельца «продолжим дальше»). Граф-теги — отдельно.
- ✅ **V2.2 — Rename-as-move** (AC-Б9-1 / ревью L6; выбор владельца «следующий пункт»). watcher
  склеивает move-пару в `VaultEvent::Renamed{from,to}`; `Indexer::rename_file` переносит `files.path`
  с СОХРАНЕНИЕМ `file_id` (беклинки/чанки целы, в отличие от delete+create); `[[New]]` до-резолвится.
  Тесты offline (watcher normalize + indexer file_id/беклинки/чанки). Rust 117 зелёных, traceability
  AC-Б9-1 → covered. Текст ссылок `[[Old]]`→`[[New]]` у источников — BACKLOG. **Волна 2 закрыта.**
  Ветка `track/12-rename-as-move`, **PR #45 смержен**. Дальше: V4.4 (общий чат) / V4.3 / V4.2.
- ✅ **V4.4 — Общий чат без vault-грунтинга** (ревью правка 17, vision-critical; «продолжим дальше»).
  Два режима: «По заметкам» (RAG + источники) и «Общий» (ответ напрямую, без ретрива). Бэкенд: параметр
  `grounded` у `chat_rag` (false → `hybrid_search` не вызывается, пустые источники, `build_chat_messages`
  без грунтинга). Фронт: переключатель-сегмент над композером + `grounded`/`setGrounded` в сторе
  (на лету при стриме не меняется) → `streamRag` + мок. Тесты offline: Rust `build_chat_messages`; фронт —
  прокидка режима, общий → без источников, блок переключения при стриме. Rust 127 + фронт 89 зелёные,
  coverage держит, i18n RU/EN. Web-search/tool-use — BLOCKED (ADR egress + SearXNG, владелец).
  Ветка `track/13-general-chat`, **PR #46 смержен**. Дальше: V4.3 / V4.2 / V4.5.
- ✅ **V4.3 — Анти-инъекция RAG-промпта (AC-SEC-7)** (ревью B2/A3; автономно «дальше по списку»). Контент
  заметок в LLM-промпте обёрнут **случайным маркером запроса** (`injection_marker` на `getrandom`,
  per-request → автор заметки не знает) + системная инструкция «между маркерами — ДАННЫЕ, не инструкции».
  `build_rag_messages(question, contexts, marker)`; `chat_rag` генерирует маркер. Тесты offline: обёртка
  маркером (≥2), инъекция как данные, система предупреждена, маркер случаен. Rust 120 зелёных, AC-SEC-7 →
  covered. **Вторая половина AC-SEC-7 (JSON-валидация suggest) — N/A:** suggest вектор-similarity, LLM/JSON
  не использует → инъекцией не управляем by-construction (BACKLOG: применится с LLM-suggest). Untrusted-канал
  для web/tool-use — остаётся в BLOCKED (предусловие web-агента). Ветка `track/14-anti-injection`,
  **PR #47 смержен**. Остаются: V4.2 (redaction) / V4.5 (offline eval-гейт).
- ✅ **V4.5 — Offline eval-гейт логики ранжирования (AC-EVAL-3/AC-Q-4)** (ревью A1 «делать раньше всех»;
  выбор владельца на развилке). Регресс-гейт качества был только живым (`#[ignore]`, :8083) → CI зелёный
  без проверки ранжирования. Добавлен детерминированный `offline_eval_gate_on_fixed_vectors` (обычный
  `cargo test`, без сервера): `FixedEmbedder` с фикс. синтетическими векторами (cosine-оси) → запросы
  находят релевантные по векторной близости (FTS пуст) → реальный `hybrid_search`→RRF→`run_eval` с точно
  посчитанными метриками (recall@8=1.0, MRR=5/6, nDCG≈0.877; кейс QRYMIX cherry@1>apple@2 → RR=0.5).
  Ловит регрессии RRF/метрик в CI. Rust 121 зелёных. AC-EVAL-3/AC-Q-4 → partial. **Реальное качество**
  (golden настоящих эмбеддингов) — BLOCKED (разовый :8083). Ветка `track/15-offline-eval-gate`,
  **PR #48 смержен**. Остаётся из Волны 4: V4.2 (redaction-layer).
- ✅ **V4.2 — Redaction-layer (AC-SEC-6)** (ревью H18; последний чистый автономный пункт очереди).
  `Redacted<T>` (модуль `redact`): Debug/Display → `<redacted>`, значение только через `expose()`.
  Crash-scrub усилен: HOME→`~` + абсолютные пути вне дома → `<path>/basename` (структура скрыта, имя
  файла оставлено), относительные/`~` не трогаются. Аудит tracing: ядро контент заметок НЕ логирует
  (проверено) → Redacted = страховка + инструмент для будущих фич. Тесты: redact (скрытие в
  Debug/Display/интерполяции) + crash (сворачивание путей). Rust 125 зелёных, AC-SEC-6 → covered.
  Широкое оборачивание Debug-полей — ✂️ инкрементально (BACKLOG). Ветка `track/16-redaction`, PR на CI.

- ✅ **Typed-frontmatter — плоские поля** (ревью H2; владелец на развилке выбрал «расширить мини-парсер»,
  закрыв NEEDS-DECISION по YAML). Парсер: плоские скаляры верхнего уровня (`progress/due/goal/evergreen/
  draft`…) → `ParsedDocument.fields`; инлайн-списки/вложенный YAML/блок-списки исключены. Таблица
  `frontmatter_fields` (миграция 003, `UNIQUE(file_id,key)` + индекс по key), индексатор наполняет
  (полная замена на файл, как теги/алиасы). Разблокирует кросс-файловые запросы (цели/stale-radar/
  Dataview). Тесты: парсер (плоские скаляры/дубль/списки/вложенность) + индексатор (запись+замена) +
  миграция (таблица). Rust 128 зелёных. Типизация значений + query-API — ✂️ с консьюмером (BACKLOG).
  Ветка `track/17-typed-frontmatter`, PR на CI.

- 📝 **Vision→AC сессия #1: Inline LLM** (выбор владельца — BLOCKED отложен под ADR, взяли vision→AC).
  Vision-фича переведена в реализуемую спеку `docs/specs/inline-llm.md`: 10 AC-IL (Given/When/Then) +
  явное «тестируем механику (мок) / НЕ тестируем качество вывода (human-eval)» + зависимости (CM6 +
  chat ADR-005) + нарезка IL-1..4. **Продуктовые решения зафиксированы владельцем:** D1 авто-ghost ВЫКЛ
  по умолчанию; D2 контекст = текущая заметка; D3 логирование принятых/отклонённых — отложено. Код не
  трогался (спека). Ветка `track/18-spec-inline-llm`, PR на CI. Дальше vision→AC (если продолжаем):
  умные шаблоны (классификатор типа заметки) / память агента (что помнить + вытеснение).

## 🧭 ОЧЕРЕДЬ ПО КРОСС-ПЛАНУ (Wave A→B→C) — активный роадмап

> Источник: мультиагентный анализ → `docs/reviews/CROSSCUT_PLAN.md` (7 линз → синтез → критика → финал,
> 35 пунктов). План принят владельцем 2026-06-04: **актуализация доков → Wave A → B → C**. Номера `#N` —
> ранги из CROSSCUT_PLAN. Фактчек скорректировал: **#8 auto_commit — выкинут (не существует)**; **#10 —
> severity↓ (pre-commit secret-scan уже есть)**; write-actor backpressure — гигиена, не OOM.

**🟢 Wave A — quick-wins (S, автономно):**
- `#2` зачистка 15 пустых теневых `' 2'`-каталогов + `.gitignore` (`* 2/`, `.nexus/`) + preflight-грэп ← делать ПЕРВЫМ (чистая карта)
- `#1` команда «Новая заметка» + `welcome.md` (сейчас пустой vault = dead-end)
- ~~`#3` de-risk `tauri build --debug`~~ **✅ 2026-06-10**: CI-джоба `bundle-debug` (push в main, ubuntu, `--bundles deb` + проверка артефакта); локально macOS `.app` собирается, dmg-шаг падает в headless-шелле (BACKLOG 🔬, проверить на живом сеансе)
- `#4` гейты от ложной зелени (`--allowOnly=false`, ignore-whitelist nextest, греп имён в check-traceability)
- `#5` синк доки с кодом (§4.3 AIClient=«план», §5.1 rebuild=«не реализ.», §2 раскладка) + AC-Q-6 авто-линт висячих упоминаний
- `#6` PRAGMA mmap/cache/temp_store + usearch F32→F16-опция
- `#7` единый source версии (вместо `0.0.0`×4) + CI-проверка синка 4 файлов
- ~~`#8`(част.) слить двойной разбор `local.json` в `open_vault`~~ **✅** (сделан ранее: `load_local_config` парсит один раз, коммент в open_vault) · ~~`#18` per-path coverage~~ **✅** (PR #103: `check-coverage.mjs` + floors `coverage-baseline.json`) · `#23` render-smoke графа · `#33` конвенция `Redacted`

**🟡 Wave B — фундамент (M/L, автономно, СТРОГИЙ порядок):**
- ~~`#13` примитив rebuild FTS5/usearch в раннере миграций~~ **✅** (`rebuild_fts`-флаг на `Migration`)
- ~~`#9` AppState-аксессоры + типизированный `AppError`~~ **✅** (`error::AppError` + `AppState::vault()`; см. журнал) · ~~`#12` Rust integration-крейт git-sync~~ **✅** (`tests/git_sync.rs`: push/pull/FF/MergeRequired через локальный bare-remote; git-identity НЕ нужна) · ~~`#28` декомпозиция `indexer/mod.rs`~~ **✅** (1302→493; подмодули links/fs/events/rag/tests)
- ~~`#10` выборочный git-стейдж~~ **✅** (PR #104: `git_commit_paths`, secret-scan по коммитимым) · `#22` пагинация `list_notes` · `#25` discriminated Buffer (под граф-во-вкладку) · ~~`#17` персист истории чата~~ **✅** · ~~`#27` DNS-rebinding гард plugin-fetch~~ **✅**
- **perf-эпик строго:** `#14` реальный токенайзер → `#15` cross-file batching (L, ломает инвариант одной задачи) → `#6` квантизация
- ~~`#11` LLM-настройки UI (11a форма + 11b hot-apply)~~ **✅** (раздел «AI / Модели» + hot-apply chat)

**🔴 Wave C — нужен владелец (ADR/инфра/решения):**
- ~~`#29` подпись/нотаризация~~ **⏸️ ОТЛОЖЕНО ВЛАДЕЛЬЦЕМ (2026-06-09)**: приложение сначала для личного
  использования (владелец сам тестирует до публичного релиза) → сертификаты Apple ($99/год)/Authenticode
  пока НЕ берём. Разблокирует `#30` updater → `#26` release.yml → `#31` E2E-смоук — всё ждёт решения о публикации.
- ~~`#16` ADR egress-хелпер ядра~~ **✅ срез 1 «Фундамент»** (ADR-005-ext E1–E10 + W1–W4 принят, док PR #105; код `net::GuardedClient` PR #106, AC-EGR-1..13 covered; срезы 2 UI/контроль · 3 cloud · 4 web — по `docs/dev/net.md`) · ~~`#21` ADR-007 планировщик джобов~~ **✅** (ADR-007 принят §0; `scheduler/` в main: очередь `jobs` + воркер + recurring/on-change/manual + backpressure S5) · `#24` развязка граф-симуляции (live-drag vs worker — sign-off) · `#19` cold-bench (живой embedder) · `#20` markdown-preview/reading-mode · `#35` vision→AC (умные шаблоны/прогресс целей)

**Жёсткие правила:** `#13` до схемо-миграций · perf `#14→#15→#6→#19` · egress док`#5`→хелпер`#16` · подпись`#29`→updater`#30` · `#16`/`#24` код автономен, но развилку — sign-off владельца.

### Прогон 2026-06-09 (сессия с владельцем — багфикс по тесту + автономный бэклог)
- **Багфикс по реальному тесту:** дайджест «завис на 15-20 мин» → idle-таймаут стрима `stream_chat`
  (90с) + сброс залипшего индикатора через `job_active`/`is_kind_busy` (PR #87, в main).
- **Аудит статуса кросс-плана** (Explore-агент по коду, не по докам): текст плана был **устарел** —
  по факту уже сделаны `#7` (`check-versions.mjs` + CI), `#13` (`rebuild_fts`-флаг), `#27`
  (`is_private_host` на plugin-fetch), `#11` (раздел «AI / Модели» + hot-apply), `#17` (chat-persist),
  `#1/#2/#4/#8/#23/#33` (Wave A). Помечены выше. **Урок: сверять статус с кодом перед взятием пункта.**
- **`#9` сделан** (типизированный `AppError`): новый `error::AppError` (`thiserror`, `Serialize`→строка,
  `#[from]` для Db/Ai/Vault/Vector/Git/Cred/Plugin/io) + аксессор `AppState::vault()` (мап-гард через
  `try_map`). 14 command-модулей сняты с `Result<T, String>`/`.map_err(to_string)` → `?`; внутренние
  хелперы (dispatch_*, get/set_setting, apply_ai) намеренно оставлены строковыми (тестируются прямо).
  Контракт фронта неизменен (JS видит строку). −68 строк, +4 теста, всё зелёное.
- **`#28` сделан** (декомпозиция `indexer/mod.rs`): 1302→493 строки, вынесены подмодули `links` (резолв
  ссылок), `fs` (обход/пути/время), `events` (watcher-петля `spawn`), `rag` (механика векторов),
  `tests`. Доступ дочерних модулей к приватным полям `Indexer`, методы `pub(super)`, `pub use
  events::spawn`. Поведение 1-в-1, 164 теста зелёные.
- **Eval-фикстура СДЕЛАНА** (владелец дал зелёный на живой прогон): разовый прогон bge-m3 @
  192.168.0.29:8083 → `recall@8=1.000, nDCG@8=0.883, MRR=0.848` (= baseline). Реальные векторы golden
  заморожены в `eval/fixture_bge_m3.json`; CI-гейт `eval_fixture_meets_baseline` гоняет их без сервера
  (`ReplayEmbedder`), регенерация `regen_eval_fixture` с guard. Закрыт пункт «Offline eval-гейт RAG».
- **`#29` подпись/нотаризация — отложено владельцем** (личное использование сначала); снято с активной
  очереди Wave C.
- **Осталось автономно в Wave B:** `#12` integration-крейт git-sync (git-identity в CI НЕ нужна —
  `GitSync::signature()` даёт дефолт), `#22` пагинация `list_notes`, `#25` discriminated Buffer,
  `#10` выборочный git-стейдж; perf-эпик `#14→#15→#6`; `#3` de-risk `tauri build`, `#18` per-path coverage.
- **Функциональный прогон LLM (gemma) СДЕЛАН** → `docs/reviews/LLM_FUNCTIONAL_REVIEW.md`. Кратко:
  модель ОК, но это **reasoning-модель**, а приложение под неё не настроено. Два бага:
  (1) `ai/chat.rs` парсит только `delta.content`, игнорит `delta.reasoning_content` → UI «мёртвая
  тишина» пока модель думает (= ощущение «зависло»); (2) на примитивах reasoning ест бюджет токенов →
  медленно и иногда ПУСТОЙ ответ. Reasoning гасится `chat_template_kwargs:{enable_thinking:false}`
  (другие способы не работают). Отдельная быстрая модель НЕ обязательна.

> ### ✅ ХВОСТЫ СЕССИИ 2026-06-09 — ЗАКРЫТЫ (сверено 2026-06-10):
> - **#93** (CRLF-фикс eval-гейта) и **#91** (интеграционные git-sync, #12) — ОБА смержены; main на
>   Windows зелёный.
> - **R1/R2 из LLM_FUNCTIONAL_REVIEW — сделаны:** R2 `without_reasoning()` (PR #94), ai.fast
>   утилитарная модель = R4 (PR #95), R1 reasoning-стрим + живая 💭-сводка (PR #96; фронт-рендер —
>   открытый PR #97, дизайн-чат). **R3 закрыт by-construction:** тело запроса `max_tokens` НЕ шлёт
>   вовсе → llama.cpp стримит до EOS/контекста, обрезки бюджетом нет (проверено `request_body`).

### Прогон 2026-06-09 (вечер) → 2026-06-10 (автономная ночь)
- ✅ **LLM-серия R1/R2/R4**: PR #94 (R2 без reasoning для примитивов), #95 (ai.fast — утилитарная
  модель, `chat_util` с fallback на gemma-fast), #96 (R1 — reasoning-стрим + живая 💭-сводка через
  `chat_util`). Фронт-рендер reasoning — PR #97 ОТКРЫТ (дизайн-чат, не трогать автономно).
- ✅ **HOME-бэкенд H1–H5 целиком**: PR #98 (get_home_data), #99 (кэш LLM-виджетов + refresh-режимы),
  #100 (Daily brief), #101 (Stale radar), #102 (Open questions + Context drift).
- ✅ **#18 per-path coverage** (PR #103: `check-coverage.mjs`, floors в `coverage-baseline.json` +
  локальный `scripts/coverage.sh`) · **#10 выборочный git-стейдж** (PR #104).
- ✅ **#16 egress — срез 1 «Фундамент» ПОЛНОСТЬЮ** (doc-first #105 → код #106): модуль
  `net::GuardedClient` + `EgressPolicy` (metadata→kill-switch→opt-in→allowlist∨private) +
  append-only `EgressAudit` (Redacted host); провайдеры + `test_ai_connection`/`probe_dim` через
  guarded (Feature::Probe — «первый egress-вектор» закрыт); фасад `AIClient{chat,chat_fast,chat_util,
  embedder,policy}` (решение владельца: все 4 провайдера); kill-switch в `AppState` взводит
  существующий `chat_cancel`; авто-allowlist явных `ai.*`-хостов (E4, offline=false дефолт);
  CI-grep-линт `check-egress.mjs` (self-test) + единственность `is_private_host`.
  **AC-EGR-1..13 → covered**, AC-EGR-14 pending (i18n-фронт, срез 2). Срезы 2–4 — `docs/dev/net.md`.
- 🧹 Хаускипинг: BACKLOG/NIGHT-PLAN сверены с кодом (этот PR); хвосты #91/#93 сняты как смерженные.

### Прогон 2026-06-10 (день, с владельцем): egress срез 2 ч.1 + News Feed бэкенд ЦЕЛИКОМ
- ✅ **Egress срез 2 ч.1** (PR #113): персист политики E5 (`egress.json` в OS config-dir, fail-safe
  дефолты), команды `get_egress_state`/`set_egress_offline`/`set_egress_feature`, блок
  «Сеть (egress)» в настройках «AI / Модели» (мгновенное применение, i18n, стейтфул-мок).
  Превью-проверка против ПРАВИЛЬНОГО воркстри (поймана ловушка `nexus-web`→старый чекаут:
  launch.json сессии, добавлена запись `nexus-be-web`:1431 + сверка cwd процесса).
  **Остаток среза 2** (чат-бейдж E9 + i18n-рендер `EgressDenied` = AC-EGR-14) — после мержа
  дизайн-PR #97 (держит чат-файлы). Попутно: cargo-deny без Docker Hub (PR #112 — security-джоба
  дважды падала на i/o-таймаутах docker-образа; бинарник с GH Releases, semantика та же).
- ✅ **News Feed: vision→AC сессия #2 + ВЕСЬ бэкенд за день** (D1–D7 решены владельцем в диалоге):
  - спека `docs/specs/news-feed.md` (12 AC-NF, 16 verified-источников — каждый фид прозвонен
    вживую; Anthropic без RSS → HN-ключи+Willison) + дизайн-handoff `NEWS_FEED_BRIEF.md` (PR #114);
  - **NF-1** (PR #115): `news/` — парсеры RSS 2.0/Atom/HF-JSON/HN-JSON → `NewsEntry`
    (quick-xml — новая workspace-зависимость, deny зелёный; выжимка/даты — мини-парсеры без
    chrono), реестр 19 источников, keyword-фильтр только для high-volume (fail-closed);
  - **NF-2** (PR #116): LLM-этап — батчи по 10, фид-контент строго между injection-маркерами,
    строгий JSON `{relevant,title_ru,summary_ru,topic}` (невалидное → видимый failed-счётчик),
    «перевод» = RU-резюме самой моделью (D1), RU-сводка дня;
  - **NF-3** (PR #117): миграция 010 (`news_items` url-UNIQUE + `news_runs` со статами/ошибками),
    дедуп с сохранением прочитанности, ретенция 30 дней, `filter_new_urls` (не жечь LLM на
    виденном), пайплайн за трейтом `FeedFetcher`, `NewsFeedHandler` (no-op без consent, S5),
    команды `get_news`/`mark_read`/`to_note`/`refresh`/`config`, «в заметку» с анти-traversal;
  - **NF-4** (PR #118): `EgressFeature::NewsFeed` — первый web-класс (дефолт ВЫКЛ, consent =
    `news.json`, `allow_private=false`), скоуповый allowlist "ai"/"news", `GuardedNewsFetcher`
    с **DNS-rebinding-гардом с пином проверенного IP** (resolve-then-connect-check без TOCTOU),
    лимиты W3 (20 с/2 МБ видимыми ошибками), регистрация в open_vault + recurring раз/сутки +
    сид «при первом открытии за день» (D3).
  **Итог: AC-NF 10 covered + 1 partial из 12; остался только UI-срез NF-5** (страница по брифу —
  ждёт макет дизайнера). Бэкенд можно пробовать: `enabled:true` в `news.json` → прогон на
  открытии vault (LLM-half оживёт с возвращением сервера владельца).
- ⏳ Отложено решениями владельца: перф-эпик #14→#15→#6 — до пересборки LLM-сервера (вместе с
  калибровкой токенайзера и перезаморозкой eval-фикстуры); web-агент/SearXNG — инфра.

### Прогон 2026-06-10 (автономная ночь, продолжение): бэклог-чистка + аудит багов
- ✅ Хаускипинг доков (PR #107) · **AC-Q-6 линт `check-dangling.mjs`** (PR #108, последний pending
  AC-Q-блока) · **#22 `list_notes(query,limit)` + `resolve_note`** (PR #109; бонус: алиасные ссылки
  кликабельны; runtime-проверка в превью) · **#3 bundle-smoke CI-джоба** (PR #110; локально macOS
  `.app` ок, dmg-шаг в headless падает → BACKLOG 🔬).
- 🔎 **Аудит багов (Explore-агент + ручная верификация КАЖДОЙ находки).** Подтверждены и
  ПОЧИНЕНЫ (этот срез): **(а) «вечные воркеры»** — `scheduler::spawn_worker` и watcher-петля
  индексатора жили вечно; каждый повторный `open_vault` плодил дубликаты (двойная индексация,
  LLM-джобы закрытого vault). Фикс: `VaultContext::lifecycle` (watcher + watch-sender), drop →
  петли гаснут; тесты `worker_loop_stops_when_shutdown_sender_dropped`,
  `event_loop_indexes_and_stops_when_sender_dropped`. **(б) паника JobHandler** оставляла джобу в
  `running` (вечный requeue без backoff) и валила воркер. Фикс: изоляция `tokio::spawn` →
  JoinError → штатный `fail()`; тест `panicking_handler_fails_job_not_stuck_running`.
  **(в) смена vault посреди чат-стрима** — осиротевший стрим продолжал жечь LLM; теперь `hydrate`
  дорезает его ДО смены ключа (хвост — в историю старого vault); тест в `chat.test.ts`.
- 🔎 **Отклонённые заявки агента** (проверены кодом, НЕ баги): rAF-буфер чата «теряется при
  unmount» — нет, стор синглтон, `done/stop` дорезают хвост синхронно; «cancelAnimationFrame
  гонится с очередью» — нет, отмена до колбэка гарантирована; usearch `save()`-fire-and-forget —
  осознанный дизайн (crash-reconcile §5.1 восстанавливает); «force-флаг индексатора рассинхронен» —
  ложь, reconcile идёт ДО создания Indexer; TOCTOU policy/send в net/ — известная микро-щель,
  приемлема (см. ADR-005-ext).
- 📎 Сверка traceability: **AC-Б10-2 → covered** (финализация в историю на `done` + видна после
  перезагрузки = сделано в #17 через localStorage per-vault; медиум отличается от задуманного
  SQLite `chat_*` — отражено в note; ChatSession-модель «много бесед» — отдельная продуктовая фича).

### 🏁 Сделано до кросс-плана (дневная сессия)
граф v2d (#44), V2.2 rename (#45), V4.4 общий чат (#46), V4.3 анти-инъекция (#47), V4.5 eval-гейт (#48),
V4.2 redaction (#49), typed-frontmatter (#50), спека inline-LLM (#51). Ночь: V1.1-1.3 тест-гейты,
V2.1/2.3/2.4, V4.1. Кросс-план — следующий крупный блок работ.

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
