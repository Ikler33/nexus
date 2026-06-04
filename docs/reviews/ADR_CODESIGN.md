# Перекрёстное проектирование: egress · планировщик · vision→AC

> Артефакт мультиагентного codesign (workflow `nexus-adr-codesign`, 22 агента, 2.38M токенов, 2026-06-04).
> Метод: 4 экспертные линзы на тему → синтез ADR → адверсариальная критика → финал → кросс-интеграция.
> **Статус (2026-06-05): РЕШЕНИЯ ВЛАДЕЛЬЦА ПРИНЯТЫ** (см. ниже). vision-волна (Похожие + Цели) реализована
> (#35, PR #66/#67). Egress зафиксирован как **расширение ADR-005**, планировщик — **ADR-007** (ARCHITECTURE §0).
> Реализация egress — после #5+#9; планировщика — после #13 + event-канал.

## Решения владельца (принято 2026-06-05)

**Egress (ADR-005-ext):** E1 egress = расширение ADR-005 (не 007) · E2 kill-switch рубит облако/web, LAN-LLM
жив · E3 уточнить формулировку AC-SEC-4 (явные `ai.*.url` разрешены, metadata всегда reject) · E4 egress OFF
по умолчанию + авто-allow хостов `local.json` · E5 политика в OS config-dir (вне vault/git) · E6 per-feature
opt-in (probe к LAN без consent) · E7 предикат metadata-блока сразу · E8 audit in-memory · E9 чат-бейдж
индикатор, без экрана журнала в v1 · E10 kill-switch рвёт стрим сразу + i18n-ошибка.

**Планировщик (ADR-007):** S1 `tokio::interval` пока vault открыт · S2 run-if-overdue catch-up · S3 первая
волна — локальные несетевые kind (News Feed после egress) · S4 триггер от завершения реиндекса · S5 жёсткий
приоритет чата · S6 числа-дефолты (в AC) · S7 backoff + max5 + видимый dead + GC · S8 **строить Tauri
event-канал** (HARD-dep) · S9 кэш по `indexed_at` · S10 offline LLM-джобы → pending.

**HARD-deps реализации:** планировщик ← #13 (rebuild-примитив) + event-канал; egress ← #5 (док-фикс §4.3) + #9
(AppError). Сетевой vision-класс (News Feed) ← egress(#16) И планировщик(#21) одновременно.

## Кросс-интеграция (зависимости · секвенсинг · решения владельца)

## Кросс-интеграция: egress(#16) × планировщик(#21) × vision→AC(#35)

Сведены три финальных DRAFT'а. Все межпунктовые связи перепроверены против `docs/reviews/CROSSCUT_PLAN.md`, `docs/NIGHT-PLAN.md`, `docs/BACKLOG.md`, `docs/reviews/BACKLOG_REVIEW.md`, `docs/design/DESIGN_BRIEF.md`, `docs/specs/inline-llm.md` и факта `.github/workflows/` (есть только `ci.yml`).

---

### 1. Картина зависимостей (перекрёстная)

Граф двухъярусный. Внизу — дешёвый **correctness-фундамент** без владельца; вверху — три дизайна и их общий заблокированный узел.

```
                 ┌─────────────────────────────────────────────┐
   ВЕРХНИЙ УЗЕЛ: │  web / cloud-fallback / News-Feed-класс       │
                 └───────▲──────────────────────▲────────────────┘
                         │ egress-половина      │ scheduled-половина
                  ┌──────┴──────┐        ┌───────┴────────┐
                  │ #16 egress  │        │ #21 scheduler  │
                  │ GuardedClient│       │ jobs+воркер    │
                  └──┬───┬───┬──┘        └──┬────┬────┬───┘
        doc-first│   │err │persist        │hard │err │event-канал
            #5 ◄─┘   │#9  │#13(если persist)│#13 │#9  │(0 Emitter)
                     │    │                 │    │
                  ┌──┴────┴─────────────────┴────┴──┐
   НИЖНИЙ ЯРУС:   │  #5 doc §4.3 · #9 AppError · #13 │  ← дёшево, автономно
                  └──────────────────────────────────┘

   #35 vision (Related Notes + Goal Progress):
   ┌───────────────────────────────────────────────────────────┐
   │ НЕ зависит НИ от #16, НИ от #21, НИ от #13, НИ от LLM.      │
   │ Стартует сразу после #5. Параллельная волна ценности.      │
   └───────────────────────────────────────────────────────────┘
```

**Уточнение по каждой связи кросс-плана (как просили):**

| Связь | Вердикт | Доказательство |
|---|---|---|
| #21 ← #13 (rebuild-примитив) | **ЖЁСТКАЯ** | CROSSCUT:71+137, NIGHT:303,313 «#13 до схемо-миграций»; jobs-таблица едет через раннер #13. Реализацию jobs **нельзя мержить раньше #13**. |
| #35 Related ← #21/#16? | **НЕТ ни на один** | max-sim из сохранённых usearch-векторов, embedder-сервер не дёргается; on-open/on-modify. BACKLOG:70, suggest-движок отгружен. |
| #35 Goal Progress ← #21/#16? | **НЕТ ни на один** | чистый SQL-read `tags`+`file_tags`+`frontmatter_fields`; пересчёт по `modify` через готовый реиндекс. |
| #35 ← #13? | **НЕТ** (инвариант) | обе фичи stateless-read, схемо-миграцию не вводят (003 отгружена). СТАНЕТ предусловием **только если** добавят `goals_cache`/`link_suggestions` (sequencing-trap). |
| probe настроек (#11b) ← egress(#16) | **ДА, обратная** | CROSSCUT:123 (MISSING THREAT rank 1), :150. `test_ai_connection(url:String)` GET-ит произвольный url из доверенного ядра ДО allowlist → **первый egress-вектор**. egress желательно **ДО/ВМЕСТЕ** с #11b. |
| §4.3-док-фикс (#5) ↔ egress(#16) | **СКЛЕЕНЫ в одно решение** | CROSSCUT:35,45,66,152; NIGHT:311,313. §4.3-фантом `AIClient` (0 в коде) egress превращает в тонкий фасад. **Док #5 — ПЕРВЫМ**, код conform-ит. |

**Три точки пересечения самих egress×scheduler** (не через vision):
1. **#13** — если владелец выберет *персист* egress-audit, это схемо-миграция через тот же раннер #13, что и фундамент `jobs` → egress втягивается в scheduler-sequencing-trap.
2. **#9 AppError** — общий сериализуемый enum ошибок: egress (AC-EGR-14 «в UI нет reqwest-строки») и scheduler (i18n статусов/причин fail джоб) тянут одну и ту же типизированную форму.
3. **Номер ADR-007** — тройная коллизия (см. §4, решение-блокер).

**Верхний узел:** `web/cloud/News-Feed`-класс заблокирован **ОДНОВРЕМЕННО** на #16 (egress) И #21 (scheduled) — BACKLOG:51,68,70; CROSSCUT:71. News Feed — *первый* заявленный scheduled-kind (PKM:310) **и** сетевой. Ни один ADR в одиночку его не открывает. Задача-трекер #29 — один артефакт на все три темы (это он).

---

### 2. Рекомендуемая последовательность

**Блок A — correctness-фундамент (параллельно, БЕЗ владельца).** `#5` doc-fix §4.3/§5.1/§2 + AC-Q-6 dangling-ref-линт · `#9` AppError-enum · `#13` rebuild-примитив раннера. Внутренний порядок неважен, кроме того что #9 и #5 нужны раньше зависимых; #13 — раньше любой схемы.

**Гейт владельца** (одним пакетом, см. §4) — собирается параллельно блоку A, т.к. весь egress/scheduler-**код** стоит за ним.

**Блок B — vision-волна** (разблокирована сразу после #5; #13 НЕ нужен): Related Notes → Goal Progress. **Идёт параллельно блокам C/D** — это её ключевое свойство.

**Блок C — egress-срез** (после #5+#9, желательно ВМЕСТЕ/ДО #11b): GuardedClient + тонкий §4.3-фасад.

**Блок D — scheduler-код** (строго ПОСЛЕ #13): `jobs`+`derived_cache`+воркер+раздельные семафоры+lifecycle+tokio `'time'`. CI sequencing-guard ловит **всех** дольщиков #13.

**Блок E — сетевые vision-фичи** (только когда #16 И #21 оба готовы): News Feed и web-класс. Карта/Противоречия — после #21 одного. inline-LLM (#35-третий) — после usable-минимума, ни #16 ни #21 не ждёт.

---

### 3. Что делать ПЕРВЫМ и почему

**ПЕРВЫМ — Блок A целиком**, и причина не в дешевизне, а в том, что эти три примитива — **общие якоря всех трёх дизайнов**, и положив их позже зависимого кода, мы пересоздаём дрейф:

- **#5 doc-fix §4.3 + AC-Q-6-линт** — §4.3-фантом `AIClient` это **общий якорь egress и vision**. egress правит §4.3 как «тонкий фасад {chat, embedder, policy}»; vision ссылается на ту же §4.3. Если код egress ляжет раньше дока — форма фасада зафиксируется кодом, а док сядет как «план» = **ровно тот дрейф «док=план vs helper»**, который уже однажды отравил планирование (фантомный `chat_messages` протёк в синтез безопасности — CROSSCUT:45,55). NIGHT:313 делает это **жёстким правилом**. Дёшево (S), автономно.
- **#9 AppError-enum** — **три** потребителя пишут ассерты/ветки против типа ошибки: egress (AC-EGR-14), #11 settings (`AiUnavailable`), #12 integration-крейт. Положив их раньше #9 — всё переписывается (CROSSCUT:140 hidden-coupling). M, автономно.
- **#13 rebuild-примитив** — **единственная жёсткая блокировка scheduler** и общий sequencing-trap: первая же миграция, перестраивающая *наполненные* FTS5/usearch без rebuild-хука, заставит пользователя руками снести `.nexus` (ломает резюмируемость). Проверено: trap ещё НЕ сработал (002 создал *пустые* derived), но #14/#17-backend/#21 его взведут. M, автономно.

Параллельно — **собрать гейт владельца** (§4), т.к. за ним стоит весь egress+scheduler-код. **Vision(#35) можно начинать сразу после #5** — он не ждёт ни #9, ни #13, ни владельца по архитектуре (только продуктовые D1–D7).

---

### 4. Сводный список решений владельца (дедуп по всем трём)

> Дедуплицировано: «персист audit/состояния» и «поведение при отказе» были в **обоих** egress и scheduler — слиты. «ADR-007 не занимать» из vision (D8) и egress поглощено единым блокером нумерации.

**🔴 БЛОКЕР #1 — общий, развязать ПЕРВЫМ.** **Номер/форма ADR.** «007» претендуют **ТРОЕ**: (а) планировщик #21 (CROSSCUT:71 прямо «ADR-007 Планировщик», NIGHT:311); (б) загрузка кода плагина/доверенный JS (BACKLOG_REVIEW A5/H4: 41,128,226 + NIGHT:133); (в) egress-черновик. **Хуже:** источник скоупа НЕ считает egress отдельным ADR — CROSSCUT:66 «**ADR-005** core-egress-хелпер», DESIGN_BRIEF:208 «(local-first, **ADR-005**)». §0 формально содержит ADR-001..006 (006=Hermes). **Решить ОДНИМ актом:** scheduler=007 (как трактуют оба плана) или нет; egress=раздел ADR-005 (как источник) или новый 008/009; vision — спека без ADR (рекомендация). Без этого три DRAFT'а дерутся за один номер.

**egress (#16):**
2. **Семантика kill-switch «офлайн»** — рубит ВЕСЬ эгресс (LAN RAG ломается) или только публичный/cloud? DESIGN_BRIEF:15,208 уже склоняют ко второму → это **подтверждение design-default**, не выбор с нуля.
3. **AC-SEC-4 конфликт (явный).** `is_private_host` написан «под AC-SEC-4» (permission.rs:239), а AC-SEC-4 (ACCEPTANCE:96 + §11:1672) перечисляет `192.168.*` среди reject-без-consent. «LAN by design» — **сознательное ослабление** клаузы. Правка формулировки («кроме явно сконфигурированного `ai.*.url`») ЛИБО consent-на-LAN.
4. **Физический носитель egress-политики.** НЕ `local.json` (git-pull, С-18) и НЕ keychain (не секрет). Назначить app-local файл / OS config-dir — иначе аргумент «не в local.json» не выполним.
5. **Дефолты+гранулярность** (слитый): (а) egress дефолт OFF (DESIGN_BRIEF:208, подтвердить); (б) allowlist пуст ПРОТИВ авто-allow из local.json; (в) гранулярность opt-in (per-feature/host/×/единый); (г) probe = эгресс-с-consent или health-check (компромисс: loopback/LAN без consent, публичный — с).
6. **Metadata-блок (169.254.169.254).** must-fix: **невозможен через реюз** `is_private_host` (один bool склеивает `192.168.*`+`169.254.*`). «LAN ок, metadata — никогда» = **новый предикат** `blocks_cloud_metadata`. Добавить сейчас ПРОТИВ отложить.

**scheduler (#21):**
7. **Движок тиков** — `tokio::interval`-пока-открыт ПРОТИВ OS-cron (+catch-up: наверстать пропущенные ПРОТИВ skip, per-kind).
8. **Список kind первой волны + каденции.** Sequencing: News Feed (сетевой) НЕ в первой волне до #16; НЕ-сетевая волна = Карта + Поиск противоречий.
9. **Backpressure-политика** при конфликте чата и scheduled-LLM-джоб (приоритет чата / честная очередь / пауза) + гранулярность on-vault-change (триггер от завершения реиндекса, не от сырого VaultEvent).
10. **Конкретные числа** (глубина канала, N воркеров, значения семафоров, отдельный лимит фоновых читателей поверх read_pool=4, throttle от батареи) + ретраи/dead-letter (max_attempts, backoff, судьба `dead`, TTL/GC — часть корректности).
11. **Event-канал (жёсткая связка).** Backend `.emit` НЕ использует (0 Emitter). Либо Tauri-event-канал — **HARD-dep** (StatusBar N/M, i18n статусов), либо первая волна kind НЕ уходит в `dead` молча. + recovery `running`-сирот.
12. **Кэш `derived_cache`:** ключ = `indexed_at` (НЕ `updated_at` — уже design-решение, подтвердить) + TTL поверх mtime + лимит размера + досверять `files.hash` для деструктивных джоб.

**ОБЩИЕ egress+scheduler (дедуп):**
13. **Персистентность audit/состояния.** egress-audit in-memory ПРОТИВ файл/`nexus.db`; persist → схемо-миграция через **#13** (связывает egress со scheduler-фундаментом) + ретенция/журнал хостов = чувствительные данные.
14. **Поведение при Denied/offline во время активного стрима/джобы.** Жёсткий отказ с i18n (зависит от #9) ПРОТИВ silent-degrade. egress: kill-switch на стриме дорезать (через взвод существующего `chat_cancel`) ПРОТИВ «дать договорить». scheduler: джобы копятся в `pending` ПРОТИВ `failed`. «No silent caps» (BACKLOG:3).
15. **UI-индикация + consent** (egress): переиспользовать существующий chat egress-индикатор (DESIGN_BRIEF:250-257); экран «журнал сети» в v1 или нет; тон consent-диалога.

**vision (#35) D1–D7:**
16. **D1** порядок волны (Related #1 → Goals #2 → inline #3). Оговорка: «near-zero» = только Rust-ядро max-sim, фронт Related всё равно net-new; Goals имеет **больше** net-new бэкенда.
17. **D2+D3+D4** (Related, слитый): include_linked=true (дискавери, accept не удаляет строку) ПРОТИВ только-несвязанные; размещение (отд. вкладка ПРОТИВ AiPanel); **D4 — блокер кода:** дефолт MIN_SCORE v1 (наследовать 0.55 / топ-N без отсечки / настройка) — нельзя оставить открытым.
18. **D5 — BLOCKING (исправлено критикой):** маркер = ТЕГ `#goal` (оба дизайн-дока) → `list_goals` JOIN-ит `tags`+`file_tags` + LEFT JOIN `frontmatter_fields` (два разных пути хранения!) ПРОТИВ frontmatter-поле `goal:` (проще, но отступление от дизайна).
19. **D6+D7** (Goals, слитый): шкала 0–100 (канон; `0≤x≤1`→×100; strip `%`) + политика битых значений (бейдж «нет прогресса» ПРОТИВ скрыть — НЕ тихий 0%, no silent caps).
20. **D8+D9** (примечания, очевидно): ADR-007 не занимать (= блокер #1); `/template` заглушка.

---

### 5. Сквозные инварианты для исполнителя

- **CI sequencing-guard (#13)** должен ловить **ВСЕХ** дольщиков, а не только jobs: `#14` re-chunk, `#17`-backend chat_*, `#21` jobs, и **egress-persist-audit** если владелец его выберет. Формулировка гейта — «schema-миграция, инвалидирующая *populated* derived».
- **#9 AppError — раньше** egress-AC-EGR-14, scheduler-i18n, #11, #12 (иначе ассерты против String переписываются).
- **doc-first (#5 → #16)** — жёсткое правило NIGHT:313; §4.3-«план» приземляется **до** кода фасада.
- **AC-Q-6 dangling-ref-линт ещё НЕ в CI** (есть только `ci.yml`) — AC-DOC-EGR частично зависит от его приземления в #5.
- **vision — отдельная разблокированная волна:** не ставить её в очередь *за* egress/scheduler; единственная её привязка к ним — общий §4.3-док (#5) и общий запрет занимать ADR-007.


---

# EGRESS — ADR-egress (#16): единая граница доверия для сетевого эгресса ядра

**Статус:** DRAFT — ожидает sign-off владельца (синтез 4 линз + учтены must-fix критики). Блокирующий sign-off по 10 пунктам, ПЕРВЫЙ из которых — НОМЕР/форма ADR (007 небезопасен — тройная коллизия + источник трактует egress как часть ADR-005).

## ADR-NNN · Единая граница доверия для сетевого эгресса ядра (`net::guarded_client`)

> **Статус:** **DRAFT — ожидает sign-off владельца.** Синтез 4 линз (архитектура / безопасность / продукт-UX / сопровождение) + учтены все must-fix критики. Блокирующий sign-off по **10 пунктам**, ПЕРВЫЙ из которых — **НОМЕР/ФОРМА ADR** (см. ниже: 007 небезопасен — тройная коллизия, а источник скоупа трактует egress как часть **ADR-005**). Заголовок помечен `ADR-NNN` намеренно — номер присваивает владелец.
>
> Реализует уже задекларированную политику §11 (`ARCHITECTURE.md:1671-1673`) и закрывает остаток `AC-SEC-4` (помечен `partial` в `traceability.json` с прямой отсылкой к «Единому egress-контролю ядра»), а не вводит новое.
>
> **Скоуп = `CROSSCUT #16` + `BACKLOG.md:70`** (единый сетевой хелпер для ВСЕХ core-эгрессов + allowlist + `is_private_host` + неотключаемый audit + per-feature opt-in + индикация ☁ + kill-switch офлайн). §4.3-фикс (`#5`) — часть ЭТОГО решения.

### Контекст

Каждый исходящий HTTP-запрос ядра сегодня строит **свой** `reqwest::Client` **внутри собственного `::new()`** — единственный общий инвариант это `redirect(none)` из `core_client_builder()` (verified `ai/mod.rs:41-43`).

| Call-site (построение клиента) | Что строит | Verified |
|---|---|---|
| `ai/chat.rs:70` | `OpenAiChatProvider` client | да |
| `ai/embedder.rs:72` | `OpenAiEmbedder` client | да |
| `ai/embedder.rs:90` | `probe_dim` — **отдельный** client мимо провайдера | да |
| `commands/settings.rs:162` | `test_ai_connection` — **отдельный** client, url **из фронта**, делает **GET** `/v1/models` (не POST) | да |
| `commands/plugin.rs:163` | `dispatch_net` — **plugin** net.fetch, свой client (15s), уже SSRF-защищён | да |

Точки **вызова** провайдеров/клиентов в composition-root: `vault.rs:103` (probe_dim), `:109` (embedder), `:131` (chat), `settings.rs:144` (hot-swap chat), `settings.rs:162` (probe). Ни `allowlist`, ни `audit`, ни `per-feature opt-in`, ни `kill-switch` НЕТ. Следствия:

1. **`N` точек = `N` мест забыть guard.** Прецедент обхода уже существует: `dispatch_net` (`commands/plugin.rs:163`) строит `reqwest::Client::builder()` мимо `core_client_builder()` (verified). Web-агент / News-Feed / cloud-fallback умножат точки. `BACKLOG.md:70`: «core-пути ходят в сеть **мимо** broker-allowlist и audit».
2. **`test_ai_connection(url: String)` принимает ПРОИЗВОЛЬНЫЙ url прямо от фронта** (не из конфига) и `GET`-ит `/v1/models` из доверенного ядра (verified `settings.rs:161-171`). Health-check на произвольный адрес = **первый и острейший egress-вектор уже сегодня** (`CROSSCUT_PLAN.md:123` — «MISSING THREAT rank 1»).
3. **§11 уже ДЕКЛАРИРУЕТ политику** (`ARCHITECTURE.md:1671-1673`: «vault не уходит без opt-in; cloud-fallback только chat с индикацией; `*.url` анти-SSRF loopback-default; **приватные/metadata-диапазоны блокируются**; pull-changed base_url требует подтверждения»; `:1651` неотключаемый audit/объём egress). `traceability.json` помечает `AC-SEC-4` как `partial`, явно отсылая остаток к «Едином egress-контроле ядра (Фундамент)» — т.е. к ЭТОМУ ADR.
4. **§4.3 `AIClient` — фантом:** `AIClient`/`cloud_fallback`/`guard_first_token`/`complete_json` дают **0 совпадений** в коде (verified grep; сам док помечает фантомом `ARCHITECTURE.md:573-577`, строка 577 прямо ссылается на `CROSSCUT #5/#16`). `VaultContext` держит `embedder` и `chat` как два независимых `Option<Arc<dyn>>` (verified `state.rs:74,77`). §4.3-фикс и egress-helper — **одно** composition-root решение (`CROSSCUT_PLAN.md:152`).
5. **Уже есть переиспользуемые паттерны:** `is_private_host` (`permission.rs:243-264`, **уже ре-экспортнут** `plugin/mod.rs:11`), net-allowlist (`permission.rs:116-122`, exact-host `==`, fail-closed), `Denied`-enum (`permission.rs:57-72`), append-only `AuditLog` с **приватным** `record()` (`broker.rs:70` — НЕ `pub`; публичны только `entries()`/`len()`/`is_empty()`, `broker.rs:79-88`).

> **Осознанная асимметрия (и где она конфликтует с AC-SEC-4 — must-fix):** `is_private_host` к ядру намеренно НЕ применён (verified doc-коммент `ai/mod.rs:38-40`) — chat/embedding LAN by design (`127.0.0.1`, `192.168.*`). **НО** `AC-SEC-4` (`ACCEPTANCE.md:96` + §11:1672) прямо перечисляет `192.168.*` среди адресов, **отклоняемых без явного согласия**, а `is_private_host` написан именно «под AC-SEC-4» (`permission.rs:239`). Значит «LAN by design» для Chat/Embed/Probe — это **СОЗНАТЕЛЬНОЕ ОСЛАБЛЕНИЕ клаузы (1) AC-SEC-4, а не её реализация**; текущий core-egress по букве AC-SEC-4 non-compliant. Этот ADR **вскрывает** конфликт и выносит его разрешение владельцу (блокер #3), а не маскирует.

### Решение

Ввести **ОДИН узкий core-egress chokepoint** — новый модуль **`net/`** (top-level, т.к. эгресс шире AI: web/News-Feed тоже через него), через который **ОБЯЗАН** проходить каждый исходящий HTTP-запрос ядра.

```text
src-tauri/src/net/mod.rs
  pub struct GuardedClient { inner: reqwest::Client, policy: Arc<EgressPolicy>, audit: Arc<EgressAudit> }
  pub enum EgressFeature { Chat, Embed, Probe }   // Web/NewsFeed/CloudFallback — позже, ВМЕСТЕ с фичей
  pub enum EgressDenied { Offline, FeatureNotEnabled(EgressFeature), HostNotAllowed(Redacted<String>) }
  pub fn unchecked() -> GuardedClient            // только #[cfg(test)] — мок-серверы без живого allowlist
```

**Дизайн-инварианты (архитектурные решения синтеза, не owner-развилки):**

1. **`inner` строится из приватизированного `core_client_builder()`** — `redirect(none)` СОХРАНЯЕТСЯ (verified `ai/mod.rs:42`). `core_client_builder()` переезжает приватной деталью внутрь `net/`; снаружи `net/` его вызов запрещён линтом (сам `net/` — whitelisted).
2. **`policy.check(host, feature)` per-request**, порядок: `kill-switch «офлайн»` → `feature opt-in` → `host ∈ allowlist`. Для `Chat/Embed/Probe` `is_private_host` **НЕ блокирует** (LAN by design) — он лишь различает приватный/публичный **для записи в audit** и для будущего web-feature. `is_private_host` импортируется из общего ре-экспорта (`plugin/mod.rs:11` — **уже готов**, отдельной работы не требует).
   > **Метадата-блок (169.254.169.254) точечно НЕВОЗМОЖЕН через реюз `is_private_host` (must-fix):** функция возвращает ОДИН `bool` на `{private | loopback | link_local}` (`permission.rs:249-255`; `is_link_local` включает metadata по комменту `:252`) — им нельзя заблокировать metadata, не заблокировав `192.168.*`. Если владелец захочет «LAN ок, metadata — никогда» (блокер #6), это **НОВЫЙ предикат** рядом (напр. `blocks_cloud_metadata`/`is_link_local`), а не реюз. Поэтому в скоуп тонкого среза metadata-блок НЕ входит.
3. **`EgressAudit` — отдельный тип** (ось `feature/host/bytes_out?/decision`), НЕ слияние с брокерским `AuditEntry` (его ось `plugin_id/method/target`, verified `broker.rs:55-60`; слияние сменило бы публичный тип и сломало бы тесты брокера). Переиспользуется **инвариант** append-only — приватный `record()` (как `broker.rs:70`), публичны только `entries()`/`len()` — + `Redacted<T>` для host (`redact.rs:15`). **`bytes_out` — best-effort `Option`** (см. ниже).
4. **`kill-switch` — НОВОЕ `AtomicBool`-поле в `AppState`** (verified: такого поля НЕТ; `chat_cancel` на `state.rs:19` — это `Mutex<Option<Arc<AtomicBool>>>`, не голый `AtomicBool`), читается per-request. **На активном стриме «офлайн» ВЗВОДИТ существующий `chat_cancel`** (`state.rs cancel_active_chat`) — per-chunk-проверка `cancel.load()` **уже есть** (`chat.rs:115`, `Ordering` импортирован `:7`). **Никакого нового механизма отмены не добавляется** (второй путь отмены = баг-ферма — must-fix).
5. **Провайдеры принимают `&GuardedClient`** вместо построения своего: меняются сигнатуры `OpenAiChatProvider::new` / `OpenAiEmbedder::new` / `probe_dim`; feature-тег передаётся при вызове.
6. **Composition-root:** `GuardedClient` строится **ОДИН раз** в `build_rag`/`build_chat` (`vault.rs:103/109/131`). **`AIClient` = тонкий фасад** `{ chat, embedder, policy }`, заменяющий два независимых `Arc` в `VaultContext` одним полем — фикс фантома §4.3 (**без** `cloud_fallback`/`guard_first_token`). Hot-swap chat (`settings.rs:140-151`) и cold embedder сохраняются. *Hot/cold-механика ортогональна egress — следить, чтобы «механический diff» не стал немеханическим.*
7. **`test_ai_connection` и `probe_dim` идут через `GuardedClient`** с `Feature::Probe` — закрывают «первый egress-вектор».
8. **CI-grep-линт:** «`reqwest::Client::builder` / `core_client_builder` вне `net/` запрещён» с **двумя явными исключениями**: (а) сам `net/` (вызывает приватизированный builder); (б) plugin-путь `dispatch_net` (`commands/plugin.rs`) — у него **своя** политика (таймаут 15s, `is_private_host`-гард `plugin.rs:151`), его миграция в `net/` **ВНЕ скоупа** этого среза, исключение сопровождается комментарием-обоснованием.

**Замок durability — это chokepoint + grep-линт, а НЕ enum:** тип `Feature` не привязан к реальному назначению (будущий код может взять `Feature::Chat` для web-запроса); гарантию единой точки даёт линт.

**Док пишется ПЕРВЫМ** (`CROSSCUT #5`: §0 + §4.3 + каскад §11/§7.9) — иначе ADR правит §4.3 в вакууме и пересоздаёт дрейф «док=план vs helper».

`bytes_out` — **best-effort `Option`** (must-fix over-eng): body строится `serde_json::json!` **внутри** провайдера ДО клиента (`chat.rs:91`), поэтому на уровне `GuardedClient` длина тела на стриме не видна без рефактора сериализации в каждый провайдер. Для не-стрим `post` — `Some(Content-Length)`; для chat-стрима — `None` (или `Some(len(messages))` при доступности). Строгий учёт отложен. Это **тело запроса** (промпт+контекст vault), НЕ ответ — privacy-правильная величина.

### Альтернативы (рассмотрены и отклонены)

- **Проверки ВНУТРЬ `core_client_builder()` без отдельного модуля** — ❌ builder не знает feature/host при построении (host приходит позже в `.get/.post`), клиент строится один раз и переживает смену allowlist/kill-switch; нельзя ни audit, ни per-feature opt-in, ни мгновенный офлайн. Ровно текущая дыра.
- **`is_private_host` к ядру «для симметрии»** — ❌ `ai/mod.rs:38-40` фиксирует LAN by design; блок мгновенно ломает RAG/chat и dev-сетап (llama `192.168.0.172`). NB: ослабляет AC-SEC-4 (1) — у владельца (блокер #3).
- **Дублировать `is_private_host`+allowlist в `net/`** — ❌ копия = две правды SSRF. Реюз тривиален (ре-экспорт `plugin/mod.rs:11` готов).
- **Точечный metadata-блок через РЕЮЗ `is_private_host`** — ❌ технически невозможен (один `bool` склеивает `private`+`link_local`, `permission.rs:249-255`). Требует НОВОГО предиката → owner-gated, не часть среза.
- **Полный §4.3 `AIClient` с `cloud_fallback`/`guard_first_token` сразу** — ❌ ЦЕЛЕВОЙ фантом (`ARCHITECTURE.md:573-577,600-617`), вне скоупа; тянет облачные провайдеры, индикацию, обрыв-внутри-стрима, отдельный opt-in (ADR-005:243).
- **Новый механизм отмены под kill-switch** — ❌ per-chunk-проверка `cancel` уже в `chat.rs:115`; офлайн = взвести существующий `chat_cancel`.
- **Слить брокерский `AuditLog` как ТИП** — ❌ другая ось (`broker.rs:55-60`, нет `bytes_out`), другой жизненный цикл; слияние сменит публичный `AuditEntry` → сломает тесты брокера. Переиспользуем инвариант (приватный `record()`), не тип.
- **Персистить audit в SQLite В ЭТОМ срезе** — ❌ sequencing-trap: схемо-миграция через rebuild-runner `#13`. In-memory как plugin-audit; «переживает рестарт» — owner-gated.
- **Политика в `.nexus/local.json`** — ❌ local.json приходит через git-pull (С-18, §11:1673); синхронизированный vault молча расширил бы границу. Нужен носитель **вне vault И вне keychain** (политика — не секрет; `settings.json` git-синхронизируем) → app-local вне git, назначить (блокер #5).
- **`lazy_static`-singleton для policy/kill-switch** — ❌ ломает composition-root, мешает тестам и hot-apply.
- **Per-provider клиенты + проверки в каждый из 5 call-sites** — ❌ дрейф гарантирован.
- **Helper ПЕРВЫМ, ADR/§4.3 потом** — ❌ одно решение (`CROSSCUT_PLAN.md:152`, NIGHT-PLAN:311); код-первым зафиксирует форму фасада, док сядет как «план».
- **Мигрировать `dispatch_net` в `net/` под общий линт** — ❌ plugin net.fetch, своя политика, уже SSRF-защищён (`plugin.rs:151`); тащить в «тонкий фасад chat+embed+probe» = необъявленное расширение скоупа. Линт даёт явное исключение; миграция — отдельный срез.

### Последствия

**Плюсы:** единственная точка построения клиента ядра (новый эгресс проходит allowlist+audit+kill-switch); закрыт `test_ai_connection`; §4.3 перестаёт быть фантомом (док=код в одном коммите); `redirect=none` сохранён; одна правда SSRF-логики (`is_private_host` из `plugin/mod.rs:11`).

**Минусы / риски (с митигацией):**

- **Blast-radius:** смена сигнатур затрагивает **5 прод-сайтов вызова** (`vault.rs:103/109/131`, `settings.rs:144/162` — две последние строят клиент мимо провайдеров) + **~7 тест-сайтов** (`chat:330`, `embedder:245`, `eval:443/535/633`, `indexer:1256`, `search:710`, `suggest:322` — все `#[cfg(test)]`, мок-серверы). → `GuardedClient::unchecked()`/fixture. Широкий, но механический diff.
- **`probe_dim` (`embedder.rs:90`) и `test_ai_connection` (`settings.rs:162`) строят клиент ОТДЕЛЬНО** — легко пропустить. → митигирует grep-линт (AC-EGR-1).
- **`bytes_out` на стриме не виден на уровне GuardedClient** (body внутри провайдера, `chat.rs:91`) → best-effort `Option`; строгий учёт отложен.
- **kill-switch на каждом эгрессе:** при дефолте ON без UI → AI «молча не работает». → типизированные ошибки (`Offline`/`FeatureNotEnabled`) + индикация. DESIGN_BRIEF:208 уже даёт дефолт egress **OFF** (локальный LLM жив).
- **Audit in-memory** не переживает рестарт → расследование постфактум неполно до owner-решения.
- **DNS-rebinding не закрыт:** allowlist по host-строке + `is_private_host` (домен → `false`, `permission.rs:262`) не ловят резолв домена в приватный/metadata-IP. Для LAN-IP из local.json неактуально; для web (если в allowlist попадёт ДОМЕН) — отложенный вектор (`BACKLOG.md:97`, `#27` `NIGHT-PLAN:305`). `Feature{allow_private=false}` для web БЕЗ DNS-resolve-guard = неполная защита.
- **kill-switch на стриме** мгновенен через взвод существующего `chat_cancel` — значит рвёт активный стрим (не «даёт договорить»; owner-развилка #10).

### Acceptance Criteria

- **AC-EGR-1** — единственный конструктор клиента ядра = `guarded_client`; CI-grep-линт падает при `reqwest::Client::builder()`/`core_client_builder()` вне `net/`. **WHITELIST:** (а) `net/`-self; (б) **явное исключение** для `dispatch_net` (`commands/plugin.rs`, своя политика, миграция ВНЕ скоупа) с комментарием-обоснованием. Тест добавляет фейк-нарушение → линт падает.
- **AC-EGR-2** — host вне allowlist → `EgressDenied::HostNotAllowed`, 0 сетевых коннектов (юнит + интеграция с мок-listener).
- **AC-EGR-3** — kill-switch=офлайн блокирует эгресс **ДО сокета** per выбранной семантике (listener обязан НЕ принять соединение).
- **AC-EGR-4** — каждый вызов (успех И отказ) → ровно одна неотключаемая запись `{feature, host, bytes_out?, decision}`; журнал append-only, `record()` приватный, публичны только `entries()`/`len()`/`is_empty()` (compile-time); host через `Redacted` (Debug не печатает); без полного URL/тела.
- **AC-EGR-5** — per-feature opt-in: Feature без включения → `FeatureNotEnabled`, audit `allowed=false`; включение → проходит. Отключение cloud-фичи не трогает локальный chat.
- **AC-EGR-6** — `test_ai_connection` (только **GET**, `settings.rs:167`) и `probe_dim` через `guarded_client`: url вне allowlist → `Denied` ДО сети, НЕ reqwest-ошибка.
- **AC-EGR-7** — `redirect=none` сохранён: тест `core_client_does_not_follow_redirects` (по **имени символа**, `ai/mod.rs`) зелёный после рефактора + кейс `302→169.254.169.254` через guarded-клиент.
- **AC-EGR-8** — `is_private_host` **НЕ ДУБЛИРУЕТСЯ**: grep → ровно одно `fn is_private_host`; `net/` импортирует из ре-экспорта (`plugin/mod.rs:11`). *Формулировка «не дублируется», НЕ «единственная SSRF-функция» — owner-metadata-блок добавит отдельный предикат рядом, что AC не нарушает.*
- **AC-EGR-9 + AC-EGR-13 (объединённый local-first smoke)** — дефолтная установка (чистый local.json) работает с локальным LLM (`127.0.0.1`/`192.168.x` для Chat/Embed/Probe НЕ блокируется) без клика по сети (при owner-дефолтах); vault/редактор/поиск title/path живут при kill-switch=офлайн (AI отключается, ядро живёт).
- **AC-EGR-10** — `bytes_out` best-effort `Option`: для не-стрим post — `Some(Content-Length)>=len(body)`; для стрима — `None`/`Some(len(messages))`. Явно: тело **запроса**, не ответ.
- **AC-EGR-11** — взведённый «офлайн» рвёт активный chat-стрим, **ВЗВОДЯ существующий `chat_cancel`** (`cancel_active_chat`), переиспользуя per-chunk-проверку `cancel.load()` (`chat.rs:115`). **Никакого нового механизма отмены.**
- **AC-EGR-12** (ортогональная hot/cold-механика — следить за раздуванием diff) — фасад строится в `build_rag`/`build_chat`, один policy; hot-swap chat переустанавливает уже-guarded клиент, cold embedder → `embedding_changed=true`.
- **AC-EGR-14** (no-silent-caps; **зависит от #9 typed errors**) — любой отказ → структурированная причина (enum), фронт рендерит i18n RU+EN; в UI нет `reqwest`/`error sending request` (сейчас `e.to_string()`, `settings.rs:169`). Блокирован отсутствием enum-канала ошибок до фронта.
- **AC-DOC-EGR** (**зависит от CROSSCUT #5 doc-first**) — §4.3 плашка-фантом (`:573-577`) заменена на реальный тонкий фасад; `cloud_fallback`/`guard_first_token` (`:600-617`) помечены «план / вне ADR»; ADR в §0 + каскад §4.3/§11/§7.9; i18n-паритет-тест (`apps/desktop/src/i18n/i18n.test.ts`) покрывает ключи `offline`/`cloud`/`denied-reason`.

### Зависимости

- **ADR-002** — переиспользуется паттерн allowlist+`is_private_host` (`permission.rs`, ре-экспорт `plugin/mod.rs:11`) и **инвариант** append-only audit (приватный `record()`, `broker.rs:70`); ADR распространяет ИХ на эгресс **ядра** (ось host/bytes_out/feature). НЕ тип `AuditEntry`.
- **ADR-005** — фасад оборачивает оба провайдера; **cloud-fallback вне скоупа** (отдельный срез). ⚠️ `CROSSCUT_PLAN.md:66` и `DESIGN_BRIEF.md:208` трактуют САМ egress-хелпер как часть **ADR-005** — см. блокер #1.
- **CROSSCUT #5 (док-фикс §4.3)** — **ЖЁСТКАЯ блокировка ПЕРЕД кодом:** §4.3-фантом надо пометить «план» ПЕРВЫМ (NIGHT-PLAN:311+313). NB: AC-Q-6 dangling-ref линт (#5) в CI **ещё НЕ приземлён** (verified: есть только `check-traceability`/`check-ignored`/`check-versions`).
- **#9 (типизированные ошибки, `AiUnavailable`)** — зависимость **AC-EGR-14**: сейчас `e.to_string()` (`settings.rs:169`); пока НЕ в коде.
- **#11b (LLM-настройки hot-apply, «Проверить связь»)** — **обратная зависимость:** probe ОБЯЗАН через guarded_client (`CROSSCUT:150,123`). Если #11b приземлится РАНЬШЕ — `test_ai_connection` уедет голым эгрессом на произвольный url. ADR-egress желательно **ДО или ВМЕСТЕ** с #11b.
- **`core_client_builder()`** (`ai/mod.rs:41`) — приватизируется внутрь `net/`, не выбрасывается; линт исключает `net/`-self.
- **`chat_cancel`** (`state.rs:19`, `begin_chat`/`cancel_active_chat`) — kill-switch на стриме взводит его. **Новое kill-switch-поле (`AtomicBool`) надо ДОБАВИТЬ** в `AppState` (`state.rs:14` — такого поля нет).
- **`Redacted<T>`** (`redact.rs:15`) — host/URL в audit (§11:1679).
- **db write-actor** (`write_actor.rs`) — **НЕ требуется** (audit in-memory); понадобится ТОЛЬКО при owner-выборе персиста → увязка со схемо-миграцией через rebuild-runner `#13`.
- **keyring v3** — канал для cloud-токенов ЕСЛИ владелец включит cloud. NB: **egress-политика — НЕ в keyring** (не секрет) и **НЕ в local.json** (git-pull) → носитель app-local вне git, назначить (блокер #5).
- **Предпосылка (НЕ достаточное условие) для** web-агент / cloud-fallback / News-Feed (`BACKLOG.md:47/70`). ⚠️ News-Feed/web-класс BLOCKED **ОДНОВРЕМЕННО** на этом ADR **И** на планировщике-ADR (`CROSSCUT_PLAN.md:71`) — этот ADR разблокирует ТОЛЬКО egress-половину. Задача `#29` трекера (ADR cross-design egress/scheduler/vision) — один артефакт на три темы.

### На sign-off владельца

1. **НОМЕР/ФОРМА ADR (блокер).** «007» претендуют **ТРОЕ** независимо: (1) **планировщик джобов** — `CROSSCUT_PLAN.md:71` прямо «ADR-007 «Планировщик»», `NIGHT-PLAN.md:311`; (2) **загрузка кода плагина / доверенный JS** — `BACKLOG_REVIEW.md:41,128,226` + `NIGHT-PLAN.md:133`; (3) этот egress-черновик. **ХУЖЕ:** первоисточник скоупа НЕ считает egress отдельным ADR-007 — `CROSSCUT_PLAN.md:66` называет его «**ADR-005** core-egress-хелпер», `DESIGN_BRIEF.md:208` тегает дефолт «(local-first, **ADR-005**)». В §0 формально `ADR-001..006` (006=Hermes). **РЕШИТЬ: egress = РАЗДЕЛ ADR-005 (как трактует источник) ИЛИ новый 008/009.** 007 для egress небезопасен и противоречит источнику.
2. **Семантика kill-switch «офлайн».** Рубит ВЕСЬ эгресс (включая LAN chat/embed → локальный RAG ломается) ИЛИ только публичный/cloud (LAN-LLM живёт)? **DESIGN_BRIEF УЖЕ склоняет ко второму:** `:15` «Локальная LLM по умолчанию; egress (облако/web) — строго opt-in», `:208` «По умолчанию OFF (local-first)». Т.е. это **подтверждение существующего design-default**, а не выбор с нуля (иначе «офлайн рубит и LAN» противоречит `DESIGN_BRIEF:208`).
3. **AC-SEC-4 КОНФЛИКТ (явный блокер — must-fix).** `is_private_host` написан «под AC-SEC-4» (`permission.rs:239`), а AC-SEC-4 (`ACCEPTANCE.md:96` + §11:1672) перечисляет `192.168.*` среди адресов, отклоняемых **без явного согласия**. «LAN by design» для Chat/Embed/Probe — **СОЗНАТЕЛЬНОЕ ОСЛАБЛЕНИЕ** этой клаузы. **РЕШИТЬ:** правка формулировки AC-SEC-4 («кроме явно сконфигурированного `ai.*.url`») ЛИБО consent-на-LAN. `traceability.json` уже помечает AC-SEC-4 `partial` с отсылкой к этому ADR.
4. **Дефолт kill-switch + allowlist на первом запуске.** egress ON/OFF (`DESIGN_BRIEF:208` → OFF, подтвердить → cloud не из коробки, локальный LLM работает). Allowlist пуст (fail-closed, трение) ПРОТИВ авто-allow хостов из `local.json` (удобно, но молча доверяет pull-изменённому URL; `AC-SEC-4`/`ACCEPTANCE.md:96` требует подтверждения pull-changed base_url).
5. **Физический носитель egress-политики (реализационный пробел).** Нельзя в `local.json` (git-pull, С-18 §11:1673) и не в keychain (не секрет; `settings.json` git-синхронизируем). **Нужен конкретный путь:** новый app-local файл вне vault / OS config-dir. Без этого аргумент «не в local.json» не выполним.
6. **`is_private_host` для ядра точечно (metadata `169.254.169.254`).** ⚠️ **must-fix:** metadata-блок **НЕВОЗМОЖЕН через реюз** — один `bool` склеивает `192.168.*` (`is_private`) и `169.254.*` (`is_link_local`, `permission.rs:250-252`). «LAN ок, metadata — никогда» требует **НОВОГО предиката** рядом. **РЕШИТЬ:** добавить новый предикат сейчас ПРОТИВ «ядру всё приватное разрешено» (metadata отложить). + Закладывать ли `Feature{allow_private=false}` для web сразу.
7. **Гранулярность opt-in.** per-feature (Chat/Embed/Probe) / per-host / per-feature×host / единый тоггл «сеть ядра». + Считать ли **probe** полноценным эгрессом (consent) или health-check. Курица/яйцо: opt-in на probe ломает UX «Проверить связь». **Предлагаемый компромисс:** probe к loopback/LAN без consent, к публичному — с consent.
8. **Персистентность audit.** in-memory (этот срез) ПРОТИВ nexus.db/файл. Персист → схемо-миграция через `#13` (sequencing) + ретенция/ротация + журнал хостов = чувствительные данные (локально, вне git). NB: «неотключаемый» = append-only + нет clear (in-memory уже даёт); «переживает рестарт» — отдельное более сильное свойство.
9. **UI-индикация + просмотр audit + тон consent.** Бейдж local/cloud/offline (переиспользовать существующий chat egress-индикатор, `DESIGN_BRIEF:250-257`, не новый StatusBar-эпик) — «облако» на cloud-fallback или на любом не-loopback? Экран «журнал сети» в v1 или audit без UI до Фазы 2 (брокерский тоже пока без UI)? + Тон consent-диалога («контекст из ваших заметок уйдёт на `<хост>`») — too-scary против too-bland (§11:1671).
10. **Поведение при `Denied` / kill-switch во время активного chat/RAG.** Жёсткий отказ с i18n-ошибкой (зависит от #9 `AiUnavailable`, пока НЕ в коде) ПРОТИВ silent-degrade. + kill-switch на стриме: дорезать немедленно (через взвод существующего `chat_cancel`, per-chunk-проверка уже есть `chat.rs:115`) ПРОТИВ дать договорить («офлайн = мгновенно» против «не рвём на полуслове»).

### На sign-off владельца (egress)
1. НОМЕР/ФОРМА ADR (блокер). «007» претендуют ТРОЕ независимо: (1) планировщик джобов (CROSSCUT_PLAN.md:71 прямо «ADR-007 Планировщик», NIGHT-PLAN.md:311); (2) загрузка кода плагина/доверенный JS (BACKLOG_REVIEW.md:41,128,226 + NIGHT-PLAN.md:133); (3) этот egress-черновик. ХУЖЕ: первоисточник скоупа НЕ считает egress отдельным ADR-007 — CROSSCUT_PLAN.md:66 называет его «ADR-005 core-egress-хелпер» и DESIGN_BRIEF.md:208 тегает egress-дефолт «(local-first, ADR-005)». В §0 ARCHITECTURE формально ADR-001..006 (006=Hermes). РЕШИТЬ: egress = РАЗДЕЛ ADR-005 (как трактует источник) ИЛИ новый номер 008/009. 007 для egress небезопасен и противоречит источнику. Заголовок помечен ADR-NNN.
2. СЕМАНТИКА kill-switch «офлайн». Рубит ВЕСЬ эгресс ядра (включая LAN chat/embed → локальный RAG ломается) ИЛИ только публичный/cloud (LAN-LLM живёт)? DESIGN_BRIEF УЖЕ склоняет ко второму: :15 «Локальная LLM по умолчанию; egress (облако/web) — строго opt-in», :208 «тоггл Разрешить облако/web... По умолчанию OFF (local-first)». Т.е. design-default = «офлайн рубит облако/web, локальный LLM жив». Подтвердить (а не выбирать с нуля), чтобы «офлайн рубит и LAN» не противоречил DESIGN_BRIEF:208.
3. AC-SEC-4 КОНФЛИКТ (новый явный блокер). is_private_host написан ПОД AC-SEC-4 (коммент permission.rs:239), а AC-SEC-4 (ACCEPTANCE.md:96 + §11:1672) перечисляет 192.168.* среди адресов, отклоняемых БЕЗ явного согласия. «LAN by design» для Chat/Embed/Probe — СОЗНАТЕЛЬНОЕ ОСЛАБЛЕНИЕ этой клаузы, т.е. текущий core-egress по букве AC-SEC-4 non-compliant. РЕШИТЬ: либо правка формулировки AC-SEC-4 («кроме явно сконфигурированного ai.*.url»), либо consent-на-LAN. traceability.json уже помечает AC-SEC-4 partial с отсылкой к этому ADR.
4. ДЕФОЛТ kill-switch + allowlist на ПЕРВОМ запуске. kill-switch/egress ON или OFF (DESIGN_BRIEF:208 → OFF, подтвердить; тогда cloud не из коробки, локальный LLM работает). Allowlist пуст (fail-closed, трение) ПРОТИВ авто-allow хостов из local.json ai.chat/ai.embedding (удобно, но молча доверяет pull-изменённому URL; AC-SEC-4/ACCEPTANCE.md:96 требует подтверждения pull-changed base_url).
5. ФИЗИЧЕСКИЙ НОСИТЕЛЬ egress-политики (реализационный пробел). Политику нельзя в local.json (git-pull, С-18 §11:1673) и не в keychain (это не секрет, а settings.json — git-синхронизируем). Нужен конкретный путь: новый app-local файл вне vault / OS config-dir. Назначить, иначе аргумент «не в local.json» не выполним.
6. ГРАНУЛЯРНОСТЬ opt-in. per-feature (Chat/Embed/Probe) / per-host / per-feature×host / единый тоггл «сеть ядра». + Считать ли probe (test_ai_connection) полноценным эгрессом (consent) или health-check. Курица/яйцо: если probe требует opt-in, UX «Проверить связь» ломается (нельзя проверить до включения). Предлагаемый компромисс для решения: probe к loopback/LAN без consent, к публичному — с consent.
7. is_private_host для ядра точечно (metadata 169.254.169.254). ВАЖНО (must-fix критики): метадата-блок НЕВОЗМОЖЕН через реюз is_private_host — один bool склеивает 192.168.* (is_private) и 169.254.* (is_link_local, permission.rs:250-252). «LAN ок, metadata — никогда» требует НОВОГО предиката рядом (blocks_cloud_metadata/is_link_local), не реюза. РЕШИТЬ: добавить новый предикат сейчас ПРОТИВ «ядру всё приватное разрешено» (metadata отложить). + Закладывать ли Feature{allow_private=false} для web сразу или до web-фичи.
8. ПЕРСИСТЕНТНОСТЬ audit. in-memory (этот срез) ПРОТИВ nexus.db/файл. Персист → схемо-миграция через #13 (sequencing) + ретенция/ротация + журнал хостов = чувствительные данные (локально, вне git). NB: «неотключаемый» = append-only + нет clear (in-memory уже даёт, broker-паттерн); «переживает рестарт» — отдельное более сильное свойство.
9. UI-ИНДИКАЦИЯ + ПРОСМОТР audit + ТОН consent. Бейдж local/cloud/offline (переиспользовать существующий chat-бейдж egress-индикатор, DESIGN_BRIEF:250-257, не новый StatusBar-эпик) — «облако» на cloud-fallback или на любом не-loopback? Нужен ли экран «журнал сети» в v1 или audit без UI до Фазы 2 (брокерский тоже пока без UI)? + Тон consent-диалога при включении облака («контекст из ваших заметок уйдёт на <хост>») — too-scary против too-bland (§11:1671 информированное согласие).
10. ПОВЕДЕНИЕ при Denied / kill-switch во время активного chat/RAG. Жёсткий отказ с i18n-ошибкой (зависит от #9 AiUnavailable, пока НЕ в коде) ПРОТИВ silent-degrade. + kill-switch на стриме: дорезать немедленно (через взвод существующего chat_cancel — per-chunk-проверка уже есть, chat.rs:115) ПРОТИВ дать договорить начатый ответ («офлайн = мгновенно ничего не уходит» против «не рвём на полуслове»).


---

# SCHEDULER — ADR-scheduler (#21): планировщик фоновых задач

**Статус:** DRAFT — ожидает sign-off владельца. Архитектурное решение фиксируется сейчас. Реализация кода таблиц и воркера РАЗБЛОКИРОВАНА строго после #13 (rebuild-примитив раннера миграций). Пункты раздела «На sign-off владельца» требуют решения до первой зависимой фичи.

## ADR-007 · Планировщик фоновых задач

**Статус:** DRAFT — ожидает sign-off владельца. Архитектурное решение фиксируется сейчас. **Реализация кода таблиц и воркера РАЗБЛОКИРОВАНА строго после #13** (rebuild-примитив раннера миграций). Пункты раздела «На sign-off владельца» требуют решения до первой зависимой фичи.

**Номер.** §0 содержит ADR-001…006 (последний — ADR-006 «Hermes», ARCHITECTURE.md:61). Следующий свободный — **ADR-007**.

---

### Контекст

Планировщика в коде **нет**: `rg 'jobs|scheduler|cron|run_at|enqueue|backpressure'` по `apps/desktop/src-tauri/src/` даёт 0 совпадений; ARCHITECTURE (1846 строк) планировщик не упоминает. Единственный фон сегодня — event-loop индексатора: `indexer::spawn` (indexer/mod.rs:588, `tokio::spawn` на :598) → `scan_vault()` → `while rx.recv()` watcher-событий.

При этом ≥5 vision-фич и весь Home стоят на персистентном планировщике (BACKLOG.md:68): **News Feed** (раз/сутки), **Карта компетенций** (full-vault раз/мес), **Поиск противоречий** (пары-кандидаты + кэш по mtime), а также on-change re-suggest. Продуктовый контракт зафиксирован в `PKM_Home_Concepts.md`: четыре режима триггеров — **on-open** (если кэш устарел), **on-vault-change** (с задержкой **~30 с**, чтобы не срабатывать на каждый символ — :29), **scheduled** (раз в сутки **пока приложение открыто** — :30,:310), **manual**; LLM-виджеты «никогда не блокируют загрузку страницы», результат кэшируется и инвалидируется «только если исходные файлы изменились после времени кэша» (:316-320).

Без единой подсистемы путь по умолчанию — **N костыльных `tokio::spawn` + N кэш-таблиц** на фичу: дублирование backpressure/ретраев/persistence, отсутствие единой точки backpressure к llama (интерактивный чат и фоновые scheduled-LLM-джобы передерутся за единственный сервер), отсутствие резюмируемости после краха.

**Что в репозитории уже есть и переиспользуется (проверено):**

| Примитив | Где | Роль для планировщика |
|---|---|---|
| Единый писатель (ADR-003) | `db/write_actor.rs:58` `transaction()` | Атомарный claim джобы `UPDATE…RETURNING` за счёт **сериализации писателя** — без новых локов |
| Read-пул (WAL, 4 коннекта) | `db/read_pool.rs:31` | Выборка готовых джоб не блокирует писателя |
| `embed_sem = Arc<Semaphore>(8)`, приватный per-Indexer | `indexer/mod.rs:46,89,313` | Единственный существующий backpressure-примитив; обобщается до per-VaultContext |
| **МОДЕЛЬ** `reconcile_vectors` | `indexer/mod.rs:509-574` | Боевая crash-recovery: chunks = источник, переэмбеддинг батчами под `embed_sem`. **NB: это in-memory diff-проход, НЕ on-disk очередь** — переиспользуем как *тело* джобы |
| mtime+size шорткат | `indexer/mod.rs:106-129` | Дешёвая инвалидация |
| `files.updated_at`+`files.indexed_at` | `001_initial.sql:12,13` | **ДВА** разных времени (см. ниже) |
| save-дебаунс скана (SCAN_CHECKPOINT=256) | `indexer/mod.rs:39,488` | Паттерн «не fsync на каждую запись» для массовых derived-операций |
| `chat_cancel` shutdown-токен | `state.rs:19` | Образец для shutdown-токена воркера |

**Острые ограничения (проверены в коде):**

1. **`migrations.rs` forward-only.** `apply()` (строки 52+) = `execute_batch(sql)` + `pragma_update(user_version)` + commit; `struct Migration{version,name,sql}` (строки 5-6) **без пост-хука**. Подтверждено: 0 совпадений `rebuild_derived/reindex/post_hook`. §5.1 (ARCHITECTURE.md:884): «примитива пересборки FTS5/usearch в раннере ПОКА НЕТ. Реализовать ДО первой схемо-миграции `chunks` — #13 (жёсткая зависимость)».
2. **`tokio` без feature `'time'`.** **КОРНЕВОЙ** workspace `Cargo.toml:20` (раздел `[workspace.dependencies]`) — только `["rt-multi-thread","sync","macros"]`; per-crate `apps/desktop/src-tauri/Cargo.toml:25` наследует `tokio = { workspace = true }`. `tokio::time::interval/sleep` для scheduled-тиков **не скомпилируется**, пока не добавить `'time'` **в корневой файл**. Watcher тикает не через tokio, а через `notify-debouncer-full` с `Duration::from_millis(400)` (watcher/mod.rs:185).
3. **Интерактивный chat-путь БЕЗ backpressure-потолка.** Инвентарь семафоров в `src/` — ровно два: `read_pool` (пул коннектов) и `embed_sem` (индексатор). `begin_chat()` **определён в state.rs:48** (вызывается из commands/chat.rs:109) — это лишь **cancel-токен** (один чат за раз, отменяет предыдущий стрим); concurrency-потолка к llama-хосту нет. *(Отмена и backpressure ортогональны — claim сюда не относится.)*
4. **`embed_sem` приватен и пер-Indexer** (создаётся в `with_rag`, indexer/mod.rs:89). Для общего потолка его надо поднять в `VaultContext` (state.rs:66) рядом с `embedder`/`chat` (state.rs:74,77). **NB:** `VaultContext` пересоздаётся при смене vault, поэтому семафор фактически **per-VaultContext** (для одного открытого vault эквивалентно «host-level»; при будущей мультивольтности термин уточнить).
5. **Lifecycle фоновой задачи негде остановить.** `indexer::spawn` (indexer/mod.rs:588) — `tokio::spawn` без сохранённого `JoinHandle`; **`commands/vault.rs:66`** вызывает его fire-and-forget; `VaultContext` (state.rs:66+) хранит `db/vectors/embedder/chat`, но НЕ хэндл и НЕ shutdown-сигнал; `lib.rs:67` строит Tauri и `.run()` **без `on_window_event`/`RunEvent::ExitRequested`**. Образец shutdown-токена рядом — `chat_cancel: Mutex<Option<Arc<AtomicBool>>>` (state.rs:19).
6. **Backend не эмитит Tauri-события вообще** (`.emit`/`Emitter` в `src/` — 0 совпадений) — канал прогресса/статуса джоб наружу + i18n RU/EN — новый объём. Это делает наблюдаемость dead-letter (см. Риски) зависимой от несуществующей инфраструктуры.
7. **`settings(key,value)`** (001_initial.sql:52) — прикладной KV (ADR-004), не место для очереди/кэша: нужны отдельные `jobs` и `derived_cache`.
8. **usearch — sibling-файл** `.nexus/vectors.usearch` вне SQLite (indexer/mod.rs:9-11), не атомарен с БД; `fts_chunks` — external-content с триггерами, нельзя `ALTER`. Любая job, перестраивающая derived, обязана дёргать те же `save()`/`reconcile_vectors()`, а не лезть в FTS напрямую.

> **Два времени в `files` (критично для инвалидации кэша).** `files.updated_at` (001:12) = **mtime** («unix ts из frontmatter ИЛИ fs»), его сравнивает mtime-шорткат `index_file` (indexer/mod.rs:116-125: `SELECT updated_at … WHERE u==mtime && s==size → ранний выход`). `files.indexed_at` (001:13) = «когда последний раз **индексировали**» — то есть когда `chunks` были перезаписаны. Derived-джоба читает **`chunks`**, поэтому свежесть её источника отслеживает **`indexed_at`**, а НЕ `updated_at`. Есть окно `indexed_at < updated_at` (файл изменён на диске, реиндекс ещё не прошёл). Плюс `updated_at` из frontmatter **не монотонен** (юзер ставит произвольную `updated:`). Вывод фиксируется в Решении и AC-SCHED-4.

> **Факт-чек спорных claim-ов против репо.** (а) «#17 chat-persist мог протащить schema-миграцию `chat_*` без #13» — **снято: ложно.** Миграций ровно три (001_initial / 002_chunks_fts / 003_frontmatter_fields), `chat_*` среди них НЕТ; chat-persist (task #27) приземлился **без схемо-миграции** (вероятно frontend-only). *(Sequencing-нюанс: если #17 в будущем доделают с backend `chat_*` через примитив #13 — он станет ещё одним дольщиком #13 ПЕРЕД jobs; гейт обязан ловить всех троих — #14 re-chunk, #17-backend, #21 jobs.)* (б) `002_chunks_fts` (chunks + FTS5 external-content + триггеры) уже приземлился обычной forward-only миграцией без rebuild-примитива — trap **не сработал**, потому что 002 СОЗДАёт **пустые** derived-структуры (наполняются индексацией, не миграцией), а не перестраивает уже наполненные. Trap про БУДУЩУЮ миграцию, обязанную пересобрать УЖЕ НАПОЛНЕННЫЕ FTS5/usearch (например #14 re-chunk при смене токенайзера). Поэтому формулировка гейта — «schema-миграция, инвалидирующая *populated* derived». (в) Фантомный `chat_messages.content plaintext` из линзы безопасности в этот ADR не вносится (таблицы нет; CROSSCUT_PLAN row 7 пометил это дрейфом).

---

### Решение

**1. Модель данных — две таблицы через раннер #13.**

```sql
-- Очередь джоб (обычная таблица; пересборки derived НЕ требует, но едет через
-- обновлённый раннер #13, чтобы зафиксировать ПОРЯДОК миграций).
CREATE TABLE jobs (
    id           INTEGER PRIMARY KEY,
    kind         TEXT    NOT NULL,
    payload      TEXT,                                   -- JSON
    state        TEXT    NOT NULL
                 CHECK (state IN ('pending','running','done','failed','dead')),
    priority     INTEGER NOT NULL DEFAULT 0,
    run_at       INTEGER NOT NULL,                       -- unix-сек; scheduled/backoff
    attempts     INTEGER NOT NULL DEFAULT 0,
    max_attempts INTEGER NOT NULL,
    last_error   TEXT,
    dedup_key    TEXT,
    created_at   INTEGER NOT NULL,
    updated_at   INTEGER NOT NULL
);
-- Идемпотентный enqueue: схлопывает дубли on-change одного файла (dedup_key=path),
-- как сейчас watcher дебаунсит шторм.
CREATE UNIQUE INDEX idx_jobs_dedup ON jobs(kind, dedup_key)
    WHERE state IN ('pending','running');
-- Выборка готовой работы. (GC done/dead обязателен — иначе индекс деградирует.)
CREATE INDEX idx_jobs_claim ON jobs(state, run_at, priority DESC);

-- Кэш LLM-виджетов (продуктовый, отдельно от jobs).
CREATE TABLE derived_cache (
    kind         TEXT    NOT NULL,
    key          TEXT    NOT NULL,                       -- widget_key / scope_hash
    value        TEXT    NOT NULL,
    source_watermark INTEGER NOT NULL,                   -- max(files.indexed_at по scope), НЕ updated_at
    computed_at  INTEGER NOT NULL,
    PRIMARY KEY (kind, key)
);
```

Время везде — unix-секунды `INTEGER`, как во всей схеме (`files.indexed_at`, `mtime_secs` indexer/mod.rs:716).

**Инвалидация кэша ключуется на `files.indexed_at`, НЕ на `updated_at`.** Derived-джоба читает `chunks`; их свежесть отражает `indexed_at` (когда chunks перезаписаны), а не `updated_at`(=mtime). Использование `updated_at` дало бы окно `indexed_at < updated_at`, в котором джоба, помеченная stale, прочитала бы СТАРЫЕ chunks; вдобавок frontmatter-`updated_at` не монотонен. Кэш считается hit, пока `max(files.indexed_at по scope) <= source_watermark`. Контракт «изменились ли источники после времени кэша → если нет, отдаём кэш» — `PKM_Home_Concepts.md:316-320`. *(«Триггер от пост-индексного сигнала» — это про КОГДА ставить джобу; «ключ `indexed_at`» — про ЧТО сравнивать. Решаются оба, раздельно.)*

**2. Воркер — один owner-loop на VaultContext поверх write-actor.** По образцу `indexer::spawn`: долгоживущий `tokio::task`, на старте делает on-open-проход (claim+run pending/overdue), затем `select!` по входам:
- **on-change** — `mpsc` от watcher → enqueue с `dedup_key=path`, `run_at=now+idle_debounce`. **Второй, более длинный idle-дебаунс ~30 с** поверх уже-нормализованных watcher-событий (PKM:29) — не путать с файловым 400 мс (watcher/mod.rs:185), у них разные цели и масштабы. Рекомендация: вешать on-change на **пост-индексный** сигнал (файл переиндексирован, `indexed_at` обновлён), не на сырой `VaultEvent` — иначе LLM-джоба прочитает `chunks` до их перезаписи индексатором.
- **scheduled** — `tokio::time::interval` (после добавления feature `'time'` в корневой Cargo.toml) будит раз/мин, выбирает jobs где `run_at<=now`. Это «раз/сутки **пока приложение открыто**» (PKM:30), НЕ системный cron. **Тик идёт ВНУТРИ `select!`** (`_ = shutdown => break, _ = interval.tick() => …`), не отдельным циклом — иначе `interval.tick().await` блокирует дренаж на закрытие на целый интервал.
- **manual / on-open** — явный enqueue из команд.

**Claim атомарен** за счёт сериализации писателя: `UPDATE jobs SET state='running', attempts=attempts+1, updated_at=? WHERE id=(SELECT id FROM jobs WHERE state='pending' AND run_at<=? ORDER BY run_at, priority DESC LIMIT 1) RETURNING …` через `WriteActor::transaction` (write_actor.rs:58). Write-actor исполняет джобы **последовательно** (один write-conn, синхронный `FnOnce(&Transaction)` — подтверждено write_actor.rs:58-70), поэтому второй claim видит `state='running'` первого внутри той же сериализации — гонки нет **без отдельных локов**. **Джоба НЕ удаляется при взятии** (lease, не delete-on-pop): на старте recovery-проход переводит зависшие `running` прошлой сессии в `pending` (процесс умер, лиз протух) или в `dead` при `attempts>=max_attempts`. **Параллелизм исполнения ТЕЛ джоб** — `buffer_unordered(N)` поверх потока claimed-id (паттерн `scan_vault`, indexer/mod.rs:478): он про конкуренцию **тел** (LLM/IO ВНЕ транзакции), не про конкуренцию claim. **Тело джобы исполняется ВНЕ транзакции**, claim/ack — короткие транзакции (как embed ДО транзакции, indexer/mod.rs:149) — иначе голод единственного писателя.

**3. Backpressure — РАЗДЕЛЬНЫЕ per-VaultContext семафоры.** Обобщить `embed_sem` (сейчас приватный per-Indexer, indexer/mod.rs:46,89) до `Arc<Semaphore>` в `VaultContext` рядом с `embedder`/`chat` (state.rs:74,77). **Отдельный семафор embed, отдельный chat** (ADR-005: разные хосты, разная латентность; §6.3:1044 «очереди embed/chat/suggest разнесены, чтобы suggest не душил чат»). Воркер берёт permit перед вызовом провайдера. **Интерактивный путь (чат/поиск по запросу пользователя) приоритетен** — обходит очередь джоб / берёт permit вперёд; scheduled-jobs оппортунистичны. Bounded job-канал (`mpsc::channel(N)`, не unbounded), чтобы on-change-шторм/scheduled-фан-аут не плодил неограниченно in-flight LLM-вызовов. Массовые derived-операции соблюдают save-дебаунс (как SCAN_CHECKPOINT) и `reconcile`-контракт. *(Отдельный лимит фоновых читателей поверх `read_pool=4` — owner-decision, см. sign-off п.6: тяжёлый scheduled read-проход не должен выедать все 4 permit и тормозить UI-SELECT.)*

**4. Lifecycle.** `JoinHandle` + shutdown-токен (по образцу `chat_cancel`, state.rs:19) в `VaultContext`; **`commands/vault.rs:66`** перестаёт быть fire-and-forget; `lib.rs:67` получает `on_window_event`/`RunEvent::ExitRequested` для дренажа очереди и остановки тика. Сейчас всего этого нет — это объём данного ADR.

**5. Очерёдность.** ADR-007 фиксируется сейчас. Реализацию таблиц+воркера ставить в очередь **ПОСЛЕ #13**. §5.1 (ARCHITECTURE.md:887-890) **обещает** persistent очередь индексации, но в коде её НЕТ — `reconcile_vectors` это in-memory diff-проход. Поэтому on-open reconcile-проход становится **первым потребителем `jobs`**, реализуя §5.1-обещание **впервые** (переиспользуя reconcile-**модель** как тело джобы), а не подключаясь к существующей очереди.

---

### Альтернативы (рассмотрены и отвергнуты)

- **N независимых `tokio::spawn` + N кэш-таблиц** (де-факто путь без ADR) — дублирует backpressure/ретраи/persistence, нет единой точки backpressure (чат и scheduled передерутся), нет резюмируемости.
- **Отдельный демон-процесс / OS-cron / launchd / Task Scheduler** — ломает single-binary упаковку Tauri, усложняет IPC/права/кроссплатформенность, противоречит local-first. Продукту нужен лишь один таймер «раз/сутки пока приложение открыто» (PKM:30,310), не системный wake.
- **Внешний крейт-планировщик** (apalis / tokio-cron-scheduler) — тянет зависимости/свой стор, дублирует write-actor+SQLite; `deny.toml`/cargo-deny гейт удорожает каждую новую зависимость.
- **Очередь/кэш в `settings` или sidecar-JSON** — settings это прикладной KV (001:52); JSON вне SQLite теряет атомарность с write-actor и транзакционную claim-семантику.
- **Один общий семафор на ВСЕ LLM-операции** (chat+embed вместе) — нарушает ADR-005 и §6.3 «очереди разнесены»; душил бы chat ради embed или наоборот.
- **delete-on-pop очередь** — краш между pop и завершением теряет задачу; lease+state переживает краш.
- **Один дебаунс** (индексаторный 400 мс для LLM-джоб) — 400 мс рассчитан на дешёвую индексацию; LLM-виджетам нужен отдельный ~30 с idle-слой (PKM:29).
- **Новый механизм пересборки derived вместо обобщения `reconcile_vectors`** — дублирует уже боевой и протестированный путь (indexer/mod.rs:509-574, тест `reconcile_restores_lost_vectors`); #13 должен ПЕРЕИСПОЛЬЗОВАТЬ reconcile-модель.
- **Ключ инвалидации кэша по `updated_at`** — derived-джоба читает `chunks`, чья свежесть = `indexed_at` (001:13), а не `updated_at`(=mtime, :12); окно `indexed_at<updated_at` → чтение старых chunks; frontmatter-`updated_at` не монотонен. Ключ — `indexed_at`.
- **Реализовать `jobs` ДО / ВМЕСТО #13** — sequencing-trap (§5.1:884, CROSSCUT_PLAN row 13): первая последующая миграция, перестраивающая populated derived, вынудит юзера снести `.nexus`.

> **Опровергнутая премиса (не повторяем).** «Backpressure как top OOM-fix через bounded-channel WriteActor» — опровергнуто (CROSSCUT_PLAN row 32): in-flight скана уже ограничен `buffer_unordered(16)` кооперативно в одной задаче, тяжёлый `Vec` векторов не перемещается в write-job. Семафор на LLM-вызовы джоб полезен как лимит **сетевой** конкуренции к llama, не как «OOM на force-скане 50k». Очередь в БД — на диске, дёшева; её размер не про OOM.

---

### Последствия

- Новая подсистема §4-уровня (jobs-воркер) + 2 таблицы через раннер #13; ARCHITECTURE §4/§5 дополняются.
- `embed_sem` поднимается из приватного поля в per-VaultContext семафор(ы); интерактивный chat впервые получает backpressure-гейт.
- В **корневой** workspace `Cargo.toml:20` добавляется feature `tokio 'time'` (per-crate наследует; правка только в корне). Слегка расширяет поверхность под cargo-deny.
- Progress/статус джоб требует канала Tauri-событий наружу (сейчас backend `.emit` не использует — 0 Emitter) + i18n RU/EN. **Dead-letter без этого канала молча теряет работу** (нарушает BACKLOG:3) — связка зафиксирована в sign-off п.8.
- Резюмируемость становится **однородной**: `jobs`(state/attempts/run_at) дают ту же гарантию, что `user_version` даёт миграциям.
- Sequencing-trap закрывается дисциплиной: реализация едет ПОСЛЕ #13; CI/ревью-гейт блокирует schema-миграцию, перестраивающую populated derived, без rebuild-хука — и **ловит всех дольщиков** (#14, #17-backend, #21), не только jobs.

---

### Acceptance Criteria

*Инварианты (фиксируются этим ADR):*

- **AC-SCHED-1 (sequencing-guard):** миграция `jobs`+`derived_cache` применяется только при наличии rebuild-примитива #13; гейт падает на попытке добавить schema-миграцию, перестраивающую *populated* FTS5/usearch, без rebuild-хука (ловит #14/#17-backend/#21). `user_version` растёт корректно, повторное открытие идемпотентно (образец `migrations_apply_and_are_idempotent`, db/mod.rs:140).
- **AC-SCHED-2 (durable enqueue + dedup):** два enqueue одного `(kind,dedup_key)` при state pending/running → одна строка (частичный UNIQUE); быстрый дабл on-change не плодит дубль (образец `atomic_save_preserves_file_id_and_backlinks`, indexer/mod.rs:854).
- **AC-SCHED-3 (crash-resume):** jobs в `running` на момент краха при следующем open подбираются reaper'ом (`running`→`pending` по таймауту / →`dead` при `attempts>=max`) и доводятся до `done` ровно один раз (образец `reconcile_restores_lost_vectors`, indexer/mod.rs:1202).
- **AC-SCHED-4 (cache-invalidation на `indexed_at`):** `derived_cache` отдаёт hit, пока `max(files.indexed_at по scope) <= source_watermark`; правка файла из scope → реиндекс поднимает `indexed_at` → stale → пересчёт; виджеты с незатронутым scope остаются на кэше. **Ключ — `indexed_at`** (свежесть chunks, 001:13), НЕ `updated_at` — иначе окно `indexed_at<updated_at` даёт чтение старых chunks (контракт PKM:316-320).
- **AC-SCHED-5 (lifecycle/shutdown):** close/смена vault останавливает тик и дренирует воркер за `<T`; tick идёт **внутри `select!`** с shutdown-токеном (не отдельным циклом); после смены нет записей в старую БД (JoinHandle+shutdown-токен в `VaultContext`).
- **AC-SCHED-6 (offline/local-first):** без llama-хоста планировщик и не-LLM jobs (reconcile, кэш по mtime) работают; LLM-jobs остаются `pending`/помечаются по политике, vault открывается без AI (как `build_rag→None`, commands/vault.rs:42 + def :92).

*Детали реализации (детализируются в impl-PR; числа/пороги — после sign-off):*

- **AC-SCHED-7 (atomic claim, no double-run):** при N конкурентных job-future поверх **одного** write-actor каждая джоба исполняется ровно раз, без двойного захвата и без `SQLITE_BUSY`. **Модель:** атомарность даёт сериализация write-actor (write_actor.rs:58); `buffer_unordered(N)` — про конкуренцию **тел** джоб ВНЕ транзакции (образец `concurrent_writes_no_busy`, db/mod.rs:204).
- **AC-SCHED-8 (retry/dead-letter):** джоба, падающая `max_attempts` раз, → `dead` с `last_error`, не зацикливается; backoff сдвигает `run_at`. **Связано с event-каналом:** `dead` без UI = нарушение no-silent-caps (BACKLOG:3) by-construction (sign-off п.7/п.8).
- **AC-SCHED-9 (run_at/scheduled):** джоба с `run_at` в будущем не исполняется до срока; scheduled-tick подбирает её ровно при `run_at<=now` (детерминированный тест с инъекцией `now`).
- **AC-SCHED-10 (idle-debounce on-change):** K быстрых правок одного файла в окне ~30 с → ровно одна on-change-джоба (последняя побеждает + дедуп) (образец `normalizes_storm_and_atomic_save`, watcher/mod.rs:224).
- **AC-SCHED-11 (backpressure, раздельные гейты):** при потолке семафора=K к llama ≤K одновременных вызовов суммарно по чату+jobs **по каждому ресурсу** (embed и chat раздельно); тест считает пик == лимиту, не `N_indexer+N_jobs`.
- **AC-SCHED-12 (chat priority + read-fairness):** при активном чат-стриме scheduled-LLM-jobs не превышают согласованную долю/паузятся; интерактивный chat/search стартует, не дожидаясь дренажа очереди; тяжёлый scheduled read-проход не выедает все 4 read-permit (read_pool.rs:31) так, чтобы UI-SELECT вставали (отдельный лимит фоновых читателей — sign-off п.6); деградация зафиксирована в BACKLOG.
- **AC-SCHED-13 (derived save-дебаунс + GC):** массовый usearch-апдейт делает `save` батчами (≤ заданной частоты, как SCAN_CHECKPOINT=256, indexer/mod.rs:39,488), не на запись, соблюдая reconcile-контракт; `done`/`dead` jobs и `derived_cache` подлежат TTL/GC (`idx_jobs_claim` деградирует на тысячах done; политика — sign-off п.7/п.9, no silent caps).

---

### Зависимости

- **#13 rebuild-примитив раннера — ЖЁСТКАЯ.** §5.1 (ARCHITECTURE.md:884): пересборки FTS5/usearch из chunks пока нет (migrations.rs forward-only; 0 совпадений `rebuild_derived/reindex/post_hook`). Реализацию `jobs` нельзя мержить раньше #13.
- **ADR-003 (write-actor)** — claim/ack/retry через `WriteActor::transaction` (write_actor.rs:58); чтение готовых джоб — `ReadPool` (read_pool.rs:31, WAL/4 коннекта).
- **ADR-005 (раздельные Chat/Embedding) + §6.3:1044** — семафоры РАЗДЕЛЬНЫЕ per-ресурс.
- **tokio feature `'time'`** — добавить в **корневой** workspace `Cargo.toml:20` (`[workspace.dependencies]`) в том же PR; per-crate `apps/desktop/src-tauri/Cargo.toml:25` наследует.
- **МОДЕЛЬ `reconcile_vectors`** (indexer/mod.rs:509-574) — переиспользуется как **тело** derived-джобы. **NB:** это in-memory diff-проход, НЕ существующая persistent-очередь — §5.1-обещанную очередь `jobs` реализует впервые.
- **Lifecycle-доработки** — `JoinHandle`+shutdown-токен в `VaultContext` (state.rs:66); не-fire-and-forget spawn (commands/vault.rs:66); `on_window_event`/`ExitRequested` (lib.rs:67).
- **Прогресс/статус** — канал Tauri-событий (сейчас backend `.emit` не использует, 0 Emitter) + i18n RU/EN; **жёстко связан с судьбой dead-letter** (sign-off п.8). Мягко связано со StatusBar N/M (CROSSCUT_PLAN row 11).
- **#22 egress-контроль — МЯГКАЯ.** **NB:** News Feed — первый заявленный scheduled-kind (PKM:310) И сетевой → НЕ-сетевые kind (Карта, Поиск противоречий) разблокированы после #13; сетевые (News Feed) — после #13 **И** #22 (BACKLOG.md:70). Сетевые kind за явным opt-in, не активировать до egress-ADR.

---

### На sign-off владельца

> «No silent caps» (BACKLOG.md:3): любой выбранный потолок/деградация фиксируется в BACKLOG, не молча.

1. **Движок scheduled-тиков:** `tokio::interval`-пока-открыт (просто; не будит закрытое приложение) ПРОТИВ OS-cron/launchd (будит, но ломает упаковку/права/кроссплатформенность/local-first). Политика продукта.
2. **catch-up при открытии:** наверстать пропущенные scheduled-прогоны (нужен persist `run_at`/`last_run`, риск лавины джоб на старте → throttle) ИЛИ skip. Per-kind.
3. **Список kind первой волны и каденции** (BACKLOG.md:51-53,68). **Sequencing:** News Feed — первый заявленный scheduled-kind (PKM:310) И сетевой → не может быть в первой волне до #22. НЕ-сетевая первая волна = Карта компетенций (локальный full-vault LLM) + Поиск противоречий (локальные эмбеддинги+LLM). Код таблицы агностичен к kind.
4. **Гранулярность on-vault-change:** маппинг widget→file-scope (daily-brief — изменения за 24 ч; прогресс целей — только `#goal`; недавние — любой modify). Триггер от ЗАВЕРШЕНИЯ реиндекса, не от сырого `VaultEvent` (рекомендация).
5. **Backpressure при конфликте чата и scheduled-LLM-jobs:** жёсткий приоритет чата (jobs ждут) vs честная очередь vs пауза scheduled во время стрима.
6. **Конкретные ЧИСЛА:** глубина bounded job-канала, N воркеров (`buffer_unordered`), значения per-VaultContext-семафоров (наследовать 8 или меньше для фона), **отдельный лимит фоновых читателей** поверх `read_pool=4` (read_pool.rs:31), throttle CPU/сети от батареи. Дефолты измеримы в AC, компромиссы — в BACKLOG.
7. **Ретраи/dead-letter:** `max_attempts`, backoff (фикс/экспонента), судьба `dead` (показывать в UI vs дропать — конфликтует с no-silent-caps), **TTL/GC** завершённых джоб (`idx_jobs_claim` деградирует на тысячах done — GC это часть корректности, не опционал).
8. **Видимость/управление в UI и зависимость от event-канала:** backend сейчас `.emit` НЕ использует вообще (0 Emitter). Решить: **либо Tauri-event-канал — HARD-dep** этого ADR (для StatusBar N/M, отмены/паузы scheduled, i18n RU/EN статусов и причин fail), **либо первая волна kind НЕ уходит в `dead` молча** (бесконечный retry+backoff для локальных, видимый `pending` для сетевых). recovery `running`-сирот: авто-requeue vs спросить пользователя (для дорогих egress-джоб владелец может хотеть подтверждение).
9. **Кэш:** (а) **ключ инвалидации — `indexed_at`** (свежесть chunks), НЕ `updated_at`; `updated_at` из frontmatter не монотонен → не watermark. (б) TTL по времени поверх mtime-инвалидации (News Feed «раз/сутки» — расписание ИЛИ кэш-TTL? если расписание — `source_watermark` для time-scoped виджетов не нужен; тонкая scope-инвалидация реально нужна Поиску противоречий / on-change re-suggest). (в) лимит размера `derived_cache`. (г) доверять ли только **секундному** mtime (две правки в одну секунду неразличимы — indexer/mod.rs:716) или досверять `files.hash` для деструктивных джоб.
10. **Offline-деградация LLM-джоб:** при недоступном провайдере — копятся в `pending` и ждут (рекомендация, как reconcile best-effort indexer/mod.rs:563 «повтор при след. открытии») ИЛИ помечаются `failed`. Per-kind opt-in для сетевых джоб (пересекается с #22).

---

### Риски

- **Sequencing-trap (главный, блокер):** реализация `jobs` мимо #13 → первая последующая миграция, перестраивающая populated derived (#14 re-chunk и т.п.), сломает `.nexus` (ручное удаление = потеря резюмируемости). Митигация: #13 — hard-prerequisite; CI/ревью блокирует такую миграцию без rebuild-хука; гейт ловит **всех** дольщиков #13 (#14, #17-backend, #21). *(Проверено: trap ещё НЕ сработал — 002_chunks_fts создал пустые derived.)*
- **Ключ инвалидации кэша на `updated_at` вместо `indexed_at`:** derived-джоба прочитает старые chunks в окне `indexed_at<updated_at`; frontmatter-`updated_at` не монотонен → кэш может вообще не инвалидироваться. Митигация: `source_watermark = max(files.indexed_at)`, единый источник; тест AC-SCHED-4.
- **tokio без `'time'`:** `interval`/`sleep` не скомпилируется — тихая ловушка на старте. Митигация: добавить feature в **корневой** Cargo.toml:20 в том же PR.
- **Backpressure-регрессия чата:** scheduled-LLM-jobs без семафора засушат интерактивный чат (commands/chat.rs:109 → begin_chat state.rs:48 сейчас без concurrency-потолка). Митигация: обобщить `embed_sem` до раздельных per-VaultContext семафоров ДО первого scheduled-LLM-kind; зафиксировать приоритет чата.
- **Двойной захват джобы:** при неверной claim-логике две job-future возьмут одну. Митигация: claim строго через `WriteActor::transaction` (сериализация писателя) + частичный UNIQUE; никаких claim вне write-actor. *(Атомарность — от сериализации, не от buffer_unordered.)*
- **Голод writer'а:** тяжёлый поток claim/finish + длинные job-транзакции конкурируют с индексацией за единственный write-actor. Митигация: claim/finish — короткие транзакции, тело job — вне транзакции.
- **Утечка/останов задачи при смене/закрытии vault:** сейчас нет хэндла (commands/vault.rs:66) и нет `on_window_event` (lib.rs:67) — воркер переживёт смену vault и будет писать в старую БД. Митигация: `JoinHandle`+shutdown-токен в `VaultContext`, дренаж на close; tick внутри `select!`.
- **Дебаунс-гонка scheduler vs индексатор:** on-change от сырого `VaultEvent` → LLM-виджет прочитает `chunks` до их перезаписи. Митигация: вешать on-change на пост-индексный сигнал.
- **catch-up лавина:** persist `run_at` + догон при открытии после долгого отсутствия → пачка джоб стартует разом. Митигация: throttle старта.
- **Read-голодание UI:** тяжёлый scheduled read-проход выедает 4 read-permit (read_pool.rs:31) → UI-SELECT'ы тормозят. Митигация: отдельный лимит фоновых читателей (owner-decision, sign-off п.6; AC-SCHED-12).
- **Save-шторм derived:** массовые usearch/FTS-операции без save-дебаунса → лавина fsync sibling-файла на 50k; usearch не транзакционен → окно потери при крахе шире. Митигация: save-дебаунс как SCAN_CHECKPOINT + reconcile.
- **Poison-job петля:** стабильно падающая на LLM-вызове джоба без attempts/backoff/dead-letter крутит llama вхолостую. Митигация: retry-политика (owner-decision).
- **Разрастание `jobs`/`derived_cache`:** done/dead/кэш копятся → деградация `idx_jobs_claim`/рост `.nexus`. Митигация: TTL/GC-вакуум; политика TTL — owner-decision (no silent caps). GC — часть корректности, не опционал.
- **`'dead'`-джобы без UI-видимости + отсутствие event-канала:** backend `.emit` не существует (0 Emitter) → наблюдаемость dead-letter требует ВСЕЙ Tauri-event-инфраструктуры, которой нет. Тихая потеря работы противоречит «no silent caps» (News Feed ушёл в dead из-за недоступного llama — пользователь не знает почему фид пуст). Митигация: **связать жёстко** — либо event-канал hard-dep, либо первая волна не уходит в dead молча (sign-off п.8).
- **i18n-долг:** статусы/ошибки/прогресс — пользовательский текст, RU/EN обязателен; backend Tauri-события не шлёт → легко упустить.
- **opt-in/egress для сетевых джоб (News Feed)** пересекается с несделанным #22: запуск сетевой джобы до egress-ADR = неаудируемый сетевой путь. Митигация: сетевые kind за явным opt-in, не активировать до #22.

### На sign-off владельца (scheduler)
1. 1. Движок scheduled-тиков: tokio::interval-пока-vault-открыт (просто; не будит закрытое приложение — News Feed «раз/сутки» проснётся при следующем открытии) ПРОТИВ OS-cron/launchd (будит, но ломает single-binary упаковку Tauri, права, кроссплатформенность, local-first). Политика продукта.
2. 2. catch-up при открытии: после долгого отсутствия — наверстать пропущенные scheduled-прогоны (нужен persist run_at/last_run, риск лавины джоб на старте → throttle) ИЛИ skip. Per-kind.
3. 3. Список kind первой волны и каденции (BACKLOG.md:51-53,68). ВАЖНО по sequencing: News Feed — ПЕРВЫЙ заявленный scheduled-kind (PKM:310) И сетевой → он НЕ может быть в первой волне до #22 (egress). НЕ-сетевая первая волна = Карта компетенций (локальный full-vault LLM) + Поиск противоречий (локальные эмбеддинги+LLM). Код таблицы агностичен к kind.
4. 4. Гранулярность on-vault-change: маппинг widget→file-scope (daily-brief — изменения за 24ч; прогресс целей — только #goal-файлы; недавние — любой modify). Триггер от ЗАВЕРШЕНИЯ реиндекса, не от сырого VaultEvent (рекомендация — иначе джоба прочитает chunks до их перезаписи).
5. 5. Backpressure при конфликте чата и scheduled-LLM-jobs: жёсткий приоритет чата (jobs ждут) vs честная очередь vs пауза scheduled во время активного стрима. «No silent caps» (BACKLOG.md:3): любой потолок/деградация фиксируется в BACKLOG.
6. 6. Конкретные ЧИСЛА: глубина bounded job-канала, N воркеров (buffer_unordered), значения per-VaultContext-семафоров (наследовать 8 или меньше для фона), отдельный лимит фоновых читателей поверх read_pool=4 (read_pool.rs:31), throttle CPU/сети от батареи. Дефолты измеримы в AC, компромиссы — в BACKLOG.
7. 7. Ретраи/dead-letter: max_attempts, backoff (фикс/экспонента), судьба 'dead' (показывать в UI vs дропать — конфликтует с no-silent-caps), TTL/GC завершённых джоб (idx_jobs_claim деградирует на тысячах done — GC это часть корректности, не опционал).
8. 8. Видимость/управление в UI и зависимость от event-канала: backend сейчас .emit НЕ использует вообще (0 Emitter в src). Решить: либо Tauri-event-канал — HARD-dep этого ADR (для StatusBar N/M, отмены/паузы scheduled, i18n RU/EN статусов и причин fail), либо первая волна kind НЕ уходит в dead молча (бесконечный retry+backoff для локальных, видимый pending для сетевых). recovery 'running'-сирот: авто-requeue vs спросить пользователя (для дорогих egress-джоб владелец может хотеть подтверждение).
9. 9. Кэш: (а) ключ инвалидации — indexed_at (свежесть chunks), НЕ updated_at; updated_at из frontmatter НЕ монотонен (юзер ставит произвольную 'updated:') → не годится как watermark. (б) TTL по времени поверх mtime-инвалидации (News Feed «раз/сутки» — расписание ИЛИ кэш-TTL? если расписание — source_max_mtime для time-scoped виджетов не нужен). (в) лимит размера derived_cache. (г) доверять ли только секундному mtime (две правки в одну секунду неразличимы — mtime в секундах, indexer/mod.rs:716) или досверять files.hash для деструктивных джоб.
10. 10. Offline-деградация LLM-джоб: при недоступном провайдере — копятся в 'pending' и ждут (рекомендация, как reconcile best-effort indexer/mod.rs:563 'повтор при след. открытии') ИЛИ помечаются failed. Per-kind opt-in для сетевых джоб (пересекается с #22).


---

# VISION — Vision→AC (#35): 1-2 дешёвые дифференцирующие фичи на готовом фундаменте

**Статус:** DRAFT — ожидает sign-off владельца. Продуктовые решения D1–D7 (+ примечания D8–D9) не зафиксированы; значения в §2 — рекомендация синтеза.

# Vision→AC (#35): «Похожие заметки» + «Прогресс целей»

> **Что это.** Перевод vision-задачи #35 («1–2 дешёвые дифференцирующие фичи на готовом фундаменте») в **реализуемую спеку**: измеримые AC, «что тестируем / что НЕ тестируем», зафиксированные продуктовые решения. Синтез 4 линз (продукт-и-дифференциация, фундамент-и-реализуемость, UX, архитектура-и-зависимости) с разрешением конфликтов.
>
> **UX/визуал — НЕ здесь:** для «Прогресса целей» описан в `docs/design/DESIGN_BRIEF.md:130` (`.prog-row`/`.prog-track`/`.prog-fill`, tabular-nums) и `PKM_Home_Concepts.md:74-76`. Для «Похожих заметок» — переиспользуется паттерн вкладки «Связи» (`components/chat/SuggestView.tsx`). Эта спека — **контракт поведения** и **тест-стратегия**.
>
> **Перепроверено против репо** (реальная раскладка `apps/desktop/src-tauri/`, НЕ фантомный `src-tauri/`): см. §6. **Все must-fix из критики учтены** (goal-marker JOIN, ADR-007 тройная бронь, mock-швы, реактивность, парс %).
>
> **Статус:** 🟡 DRAFT — продуктовые решения **D1–D7 ждут sign-off владельца** (§2; D8–D9 — примечания). Готово к нарезке после фиксации.

## 0. Решение (TL;DR)

Взять **две** дешёвые фичи на уже отгруженном фундаменте — обе **без планировщика(#21), без egress(#16), без LLM**:

1. **«Похожие заметки» (Related Notes)** — *второй потребитель* уже работающего max-sim движка `suggest`, считающего из сохранённых usearch-векторов **без обращения к embedder-серверу**. **0 нового AI-кода ядра.** Рекомендуется **первой**.
2. **«Прогресс целей» (Goal Progress)** — *первый кросс-файловый query-консьюмер*, который **JOIN-ит `file_tags`↔`tags` (маркер-тег `#goal`) с LEFT JOIN `frontmatter_fields` (значение `progress`)**. Чисто SQL-read, **нулевая вероятностная поверхность** → детерминированный зелёный CI.

**Inline-LLM** остаётся **третьим** (спека `docs/specs/inline-llm.md` готова, объём M–L). Полные **«Умные шаблоны»** **отвергнуты** как «дешёвые сейчас»; их единственный дешёвый срез (ретрив «топ-3 похожих») = Related Notes.

## 1. Скоуп v1 и non-goals

**v1 (в этой спеке):**
- **Related Notes:** команда `get_related_notes(path, limit)` — тонкая обёртка/флаг `include_linked` над `suggest::get_link_suggestions` (`vector::get_vector` + `search_filtered`, max-sim агрегация). Фронт: клон `stores/suggest.ts` + секция/вкладка по образцу `SuggestView.tsx`. Вставка `[[wikilink]]` из панели.
- **Goal Progress:** команда `list_goals()` — **JOIN `tags`+`file_tags` (`WHERE tags.name='goal'`) LEFT JOIN `frontmatter_fields` (`key='progress'`)**, парс/валидация `progress`→0..100 **на чтении**. Фронт: вкладка «Цели» в `AiPanel`, прогресс-бары по `DESIGN_BRIEF:130`. Клик → открыть заметку.

**Non-goals v1 (отдельные срезы/спеки, «no silent caps»):**
- **LLM-обоснование/режим-2 Suggest** — за eval-гейтом Ф1-10. Отдельно.
- **Полные «Умные шаблоны»** (классификатор эвристика→LLM + обучение) — отдельная vision→AC спека (как `inline-llm.md §7`); здесь дешёвый ретрив-срез = Related Notes; `/template` — заглушка.
- **Кэш-таблица** `link_suggestions`/`goals_cache`, персист, обучение — позже (тянет #21 / sequencing-trap миграций #13). Обе фичи на первом заходе — **stateless-read**.
- **Типизация frontmatter на уровне хранения** — значения остаются сырым TEXT; парс на чтении. Будущий ADR.
- **Точная вставка по позиции курсора CM6** (Related) — v1 дописывает в конец доки; cursor-precise — позже.
- **Поле `due`** — фигурировало в раннем наброске запроса, но AC/назначения нет → **исключено из v1-запроса** (добавить при появлении требования).
- **Авто-триггеры / scheduled-пересчёт** — нет; on-open/on-modify ленивый расчёт.

## 2. Продуктовые решения — ⚠️ ЖДУТ SIGN-OFF ВЛАДЕЛЬЦА

> В отличие от `inline-llm.md` (где D1–D5 ✅), здесь решения **не зафиксированы**. Значения ниже — **рекомендация синтеза**.

| ID | Решение | Рекомендация (на подтверждение) |
|----|---------|----------------------------------|
| **D1-ВЫБОР** | Порядок волны | **Related #1 → Goals #2 → inline-LLM #3** (но «near-zero» = только ядро Related, не фича целиком) |
| **D2** | Related: показывать связанные? | **`include_linked=true`** (дискавери); влияет на accept (строка не удаляется) |
| **D3** | Related: UX-размещение | Отдельная вкладка/секция рядом с «Связи» |
| **D4** | Related: **дефолт порога v1** | Топ-N без жёсткой отсечки (≠ слепо 0.55); порог в настройку позже |
| **D5** | Goals: маркер цели | Тег **`#goal`** → JOIN tags+frontmatter_fields (альт.: поле `goal:`) |
| **D6** | Goals: шкала + маппинг | **0–100**; `0≤x≤1`→×100; strip `%` |
| **D7** | Goals: политика битых значений | Строка без бара + бейдж «нет прогресса» ИЛИ скрыть — **явно, не тихий 0%** |
| **D8** *(примечание)* | Формальный ADR-007? | **НЕ занимать 007** (тройная бронь); спека без ADR |
| **D9** *(примечание)* | Smart-templates сейчас? | Нет — `/template` заглушка; ретрив покрыт Related |

**Обоснование ключевых:**
- **D1.** Related первой: near-zero ядро (обёртка над отгруженным движком). НО фронт требует net-new (стор+вью+i18n+D3), Goals — больше net-new бэкенда (JOIN+парс+реактивность). «Related ≈ бесплатно» неверно.
- **D2.** `include_linked=true` иначе Related дублирует Suggest. Дискавери = «всё близкое для навигации».
- **D5 (BLOCKING-исправлено).** Оба дизайн-дока (`PKM_Home:76`, `DESIGN_BRIEF:130`) говорят **тег `#goal`** + поле `progress` 0–100. Тег → `file_tags`/`tags`; `progress` → `frontmatter_fields` — **разные таблицы** → запрос ОБЯЗАН их JOIN-ить (см. §5). Прежний набросок `WHERE key IN ('goal',…)` по `frontmatter_fields` матчил бы только литеральный скаляр `goal:`, а не тег — **дефект исправлен**.
- **D6.** `progress:0.5` (`parser/mod.rs:454`) — демо raw-passthrough, НЕ решение шкалы (хранится сырым TEXT, `003:10`).

## 3. Acceptance Criteria (Given/When/Then)

> **Тестируем МЕХАНИКУ** детерминированно (Related — **Rust** `MockEmbedder` для выдачи/сортировки; Goals — SQLite-фикстур), НЕ семантическое качество. Нумерация `AC-RN-*`/`AC-GP-*`; переедут в `docs/acceptance/ACCEPTANCE.md` + `traceability.json`. При переносе ссылаться на **якоря/ID пунктов** дизайн-доков, а не на номера строк (markdown-строки смещаются).

### Related Notes
- **AC-RN-1 (расчёт без сервера)** *When* `get_related_notes(path)` *Then* результат из usearch-векторов **БЕЗ embedder-сервера**, по убыванию max-sim, cap 20. Доказывается **БЭКЕНД-тестом на Rust `MockEmbedder{dim:16}`** (`embedder.rs:178`; путь как `suggest/mod.rs:215,232`) — `#[cfg(test)]`-структура внутри крейта. **NB:** JS-мок `tauri-api.ts:267-268` — ДРУГОЙ шов (канвенные данные, max-sim не исполняет); ассерт «embedder не вызван» — свойство Rust-теста, не фронт-мока.
- **AC-RN-2 (включает связанные — дискавери)** *Given* `include_linked` *Then* связанная B **ПРИСУТСТВУЕТ** — в отличие от `get_link_suggestions` (`suggest/mod.rs:84,148-155`).
- **AC-RN-3 (исключает сам файл)** *Then* A НИКОГДА не в выдаче (`suggest/mod.rs:52,64,84`).
- **AC-RN-4 (пусто без индекса)** *Given* `vectors=None` *Then* пусто без краша (`commands/suggest.rs:21-23`).
- **AC-RN-5 (порядок СОРТИРОВКИ — НЕ семантика)** *Given* фикстуры, пересекающиеся по **сырым байтам** контролируемо (`mock_vec` = частоты байтов→L2, `embedder.rs:183-190` → близость = байты, не тема) *Then* более байт-близкая строго ВЫШЕ. Тестируем **плюмбинг сортировки/агрегации**, не «темы». Семантика — live `#[ignore]` :8081 (`suggest/mod.rs:298-347`); опц. `FixedEmbedder` (`eval/mod.rs:301-307`) для осмысленного ранжирования без сети.
- **AC-RN-6 (вставка `[[wikilink]]`, поведение строки)** *When* «вставить связь» *Then* в буфер A дописывается `[[B]]`, dirty (`updateBufferDoc`, `workspace.ts:130`; вставка как `stores/suggest.ts:55-60`). **Развилка:** существующий `accept` вызывает `dismiss` (`suggest.ts:62`) и убирает строку — для **дискавери** (D2) строка остаётся → `Related.accept` **НЕ** вызывает `dismiss`. Cursor-precise — НЕ в v1.
- **AC-RN-7 (i18n)** заголовок/пустые/loading — RU/EN (паттерн `suggest.*`); без хардкода.

### Goal Progress
- **AC-GP-1 (кросс-файловый сбор через JOIN тег+поле)** *Given* заметки с **тегом `#goal`** *When* `list_goals()` *Then* `{path,title,progress:0..100|null}` для заметок с тегом; `progress` — LEFT JOIN `frontmatter_fields` (`key='progress'`). **Первый** кросс-файловый консьюмер, соединяющий `tags`+`file_tags` (фильтр по `idx_file_tags_file`/`tags.name`, `001:32-41,60`) с `frontmatter_fields` (`idx_frontmatter_fields_key`, `003:16`). Дефолтный cap (как `min(20)`, `commands/suggest.rs:24`); жёсткий `LIMIT`-как-AC не нужен (`BACKLOG:135` — пагинация отложена).
- **AC-GP-2 (парс 0–100 + валидация + strip %, no silent caps)** *Given* `progress="80"`/`"80%"`(strip)/`"0.5"`→50 (D6) *Then* `progress=80|50`; *Given* тег есть, `progress` отсутствует/>100/нечисловое (`"WIP"`,`"треть"`) *Then* помечена «без валидного прогресса» (показ по D7), НЕ тихий 0%. Парс из сырого TEXT (`003:10`; parser снимает кавычки+trim, НО **не `%`** — `parser/mod.rs:305` → strip `%` на чтении).
- **AC-GP-3 (живой пересчёт + явный шов реактивности)** *Given* `progress` изменён + реиндекс (`indexer:197-204`, replace `UNIQUE(file_id,key)`) *When* сигнал `modify` **любого** goal-файла *Then* список инвалидируется, `list_goals` перезапущен, значение обновлено — БЕЗ #21 (`DESIGN_BRIEF:108`, `PKM_Home:74`). **NB-шов:** Suggest реагирует на `activePath`; кросс-файловому списку нужен триггер на modify *любого* (не активного) файла — **новый шов**, специфицировать подписку на событие индексатора.
- **AC-GP-4 (детерминизм/зелёный CI)** *Given* SQLite-фикстур *Then* стабильно; LLM/embedder/сеть НЕ участвуют (`DESIGN_BRIEF:130`). Тестируется **полностью**.
- **AC-GP-5 (информативное пустое состояние)** *Given* нет заметок с `#goal` *Then* пример конвенции (тег `#goal` + поле `progress`, RU/EN), не голый вид.
- **AC-GP-6 (навигация)** *When* клик *Then* заметка открывается.
- **AC-GP-7 (i18n)** заголовок, подписи, проценты-mono (tabular-nums), пустое состояние — RU/EN.

### Общий
- **AC-X-1 (офлайн-инвариант)** *Given* выключены все LLM/embedder-серверы *Then* обе работают полностью (max-sim из векторов / SQL-read tags+frontmatter_fields), подтверждая local-first.

## 4. Измеримый успех / что НЕ тестируем

- **Тестируем (зелёный CI):** AC-RN-1..7, AC-GP-1..7, AC-X-1. Related — Rust `MockEmbedder dim 16` (механика выдачи/сортировки). Goals — SQLite-фикстур. Механика: расчёт-без-сервера, семантика дискавери (include_linked), сортировка, вставка (+поведение строки), парс/валидация/strip %, реактивность, навигация, empty-state, i18n, офлайн.
- **НЕ тестируем зелёным CI:**
  - **Related:** качество/полезность списка для человека — **human-eval** (как `inline-llm.md §4`). Абсолютный `MIN_SCORE` — калибровка Ф1-10. Семантическое 'близка/далека' — невоспроизводимо на байт-mock (`mock_vec` = байты, не темы) → только live `#[ignore]` :8081.
  - **Goals:** вероятностного нет (без LLM) — вся механика детерминирована.

## 5. Зависимости / архитектура

| Фича | #21 | #16 | LLM | Таблицы / прочее |
|------|:---:|:---:|:---:|--------|
| **Related Notes** | **НЕТ** (on-open/on-modify) | **НЕТ** (max-sim из векторов, `suggest/mod.rs:4-7`) | **НЕТ** | RAG-индекс (Ф1-5) |
| **Goal Progress** | **НЕТ** (modify→реиндекс) | **НЕТ** (SQL-read) | **НЕТ** | `tags`+`file_tags`+`frontmatter_fields`+`files` |
| *Inline-LLM (#3)* | НЕТ | НЕТ (`inline-llm.md §5`) | да | chat-стрим **готов** (`ai/chat.rs:48`) |

- **Related:** `get_related_notes` поверх `vector::search_filtered`/`get_vector` (`vector/mod.rs:128,149`) + max-sim (`suggest/mod.rs`). Регистрация рядом с `get_link_suggestions`. Фронт — клон стора+вью.
- **Goals (ИСПРАВЛЕННЫЙ запрос):** маркер `#goal` — ТЕГ (пишется `indexer:225-233` в `file_tags`/`tags`, миграция `001:32-41`), значение `progress` — frontmatter-скаляр (`indexer:197-204` в `frontmatter_fields`, миграция `003`). Запрос:
  ```sql
  SELECT f.path, f.title, ff.value AS progress_raw
  FROM files f
  JOIN file_tags ft ON ft.file_id = f.id
  JOIN tags t       ON t.id = ft.tag_id AND t.name = 'goal'
  LEFT JOIN frontmatter_fields ff ON ff.file_id = f.id AND ff.key = 'progress'
  WHERE f.is_deleted = 0;
  ```
  Парс/валидация/strip `%` `progress_raw`→0..100 **на чтении** (типизация НЕ требуется на уровне хранения).
- **#13 инвариант:** ни одна фича не вводит схемо-миграцию (003 отгружена; `frontmatter_fields` — plain-таблица, наполняется транзакционно per-file, НЕ derived usearch/FTS) → **#13 не предусловие**. Добавят `goals_cache` → станет.
- **ADR:** формальный ADR-007 **НЕ занимать** (§«ADR-007» в consequences) — тройная бронь.

## 6. Проверка claim-ов против репо (verification log)

| Claim | Вердикт | Доказательство (file:line) |
|-------|:------:|----------------------------|
| max-sim из сохранённых usearch-векторов, без embedder-сервера | ✅ TRUE | `suggest/mod.rs:4-7,57-65`; `vector/mod.rs:149` `get_vector`, `:128` `search_filtered` |
| Suggest вычитает связанные | ✅ TRUE | `suggest/mod.rs:84,148-155` |
| Rust `MockEmbedder{dim:16}` — `#[cfg(test)]` в крейте; `mock_vec`=частоты байтов | ✅ TRUE | `embedder.rs:178,183-190`; используется `suggest/mod.rs:215,232` |
| JS-мок `tauri-api` — ДРУГОЙ шов (не исполняет max-sim) | ✅ TRUE (исправлено) | `tauri-api.ts:267-268` ≠ Rust-тест; критика права |
| **Маркер `#goal` — ТЕГ (file_tags/tags), НЕ frontmatter_fields** | ✅ **TRUE (BLOCKING-исправлено)** | теги: `indexer:225-233`, миграция `001:32-41`; `progress`: `indexer:197-204`, `003`. Разные таблицы → §5-запрос переписан на JOIN |
| `progress` — frontmatter-скаляр, сырой TEXT | ✅ TRUE | `003:10` `value TEXT NOT NULL`; parser снимает кавычки+trim, **не `%`** (`parser/mod.rs:305`) |
| `frontmatter_fields` пишется, но НЕТ кросс-файлового консьюмера | ✅ TRUE | пишет `indexer:197-204`; единственный SELECT — single-file `WHERE file_id=?1` (`indexer:802`) |
| Конфликт шкалы 0–1 vs 0–100 | ✅ РАЗРЕШЁН | `PKM_Home:76` «0–100», `DESIGN_BRIEF:130`; `progress:0.5` (`parser:454`) — raw-демо |
| chat-стрим переиспользуем для inline-LLM | ✅ TRUE | `ai/chat.rs:48` `stream_chat` |
| #21 планировщик / #16 egress отсутствуют | ✅ TRUE | `CROSSCUT_PLAN:71` «0 scheduler/JobQueue в src»; `BACKLOG:70` |
| **ADR-007 контендится ТРОЙНО** | ✅ **TRUE (исправлено)** | #21: `NIGHT-PLAN:311`, `CROSSCUT_PLAN:71`; плагин-код/editor-ext: `NIGHT-PLAN:133`, `BACKLOG_REVIEW:41,128,226` |
| Goal Progress — DYNAMIC-виджет Home, не панель-сосед Suggest | ✅ TRUE | `DESIGN_BRIEF:130` `.prog-row в .h-card`; `PKM_Home:74` «динамический» → размещение в AiPanel = решение D3 |
| LIMIT-как-AC преждевременен | ✅ TRUE | `BACKLOG:135` пагинация тяжёлого IPC отложена, «объёмы пока малы» |
| `suggest.accept` → `dismiss` (убирает строку) | ✅ TRUE | `stores/suggest.ts:62` → для дискавери AC-RN-6 отменяет dismiss |
| `files.title` есть для JOIN | ✅ TRUE | `001:10` |

## 7. Открытые вопросы

- **D1–D7** — ждут владельца (§2); **D8–D9** — примечания (очевидный ответ).
- **Шов реактивности Goal Progress** — подписка на событие индексатора на modify любого goal-файла (AC-GP-3); конкретный механизм специфицировать при нарезке.
- **Дефолт порога Related (D4)** — зафиксировать на v1 (рекомендация: топ-N без отсечки), не оставлять открытым.
- **«Умные шаблоны» (классификатор)** — отдельная vision→AC спека (эвристика→LLM, пороги, обучение), как `inline-llm.md §7`.
- **Поле `due`** — добавить AC при появлении продуктового требования (сейчас вне запроса).
- **Типизация frontmatter на уровне хранения** — будущий ADR (НЕ 007); на первом заходе парс-на-чтении достаточно.

### На sign-off владельца (vision)
1. D1-ВЫБОР (порядок волны): подтвердить «Related Notes #1 + Goal Progress #2, inline-LLM #3» ИЛИ переставить. Рекомендация: Related первой. ОГОВОРКА: «near-zero новый код» относится строго к Rust-движку max-sim (бэкенд = обёртка); фронт Related всё равно требует новый стор+вью+i18n+размещение (D3), а Goals имеет больше net-new бэкенда (JOIN-запрос+парс+шов реактивности). «Related ≈ бесплатно» — НЕверно; бесплатно лишь ядро.
2. D2 (Related — семантика): показывать ли уже-связанные заметки (include_linked=true → «дискавери, всё близкое») ИЛИ только несвязанные (аудит связей). Меняет AC-RN-2 и поведение accept (AC-RN-6: при дискавери строка НЕ удаляется после вставки). Рекомендация: include_linked=true, иначе фича дублирует Suggest.
3. D3 (Related — UX-размещение): отдельная панель/вкладка ИЛИ секция рядом с «Предложениями связей» в AiPanel. Влияет на риск «две похожие панели». Рекомендация: отдельная вкладка/секция рядом с «Связи».
4. D4 (Related — порог дискавери, дефолт v1 ОБЯЗАТЕЛЕН): какой MIN_SCORE на первом заходе — унаследовать 0.55 (риск отсечь валидные RU-близкие, ADR-005) ИЛИ 0 (показывать топ-N без отсечки) ИЛИ настройка. НЕЛЬЗЯ оставить нерешённым (блокер кода). Рекомендация: топ-N без жёсткой отсечки на v1 (дискавери-режим терпим к шуму) + вынести порог в настройку позже; калибровка — eval Ф1-10.
5. D5 (Goals — маркер цели, BLOCKING-исправлено): подтвердить маркер = ТЕГ #goal (оба дизайн-дока: PKM_Home:76, DESIGN_BRIEF:130) → list_goals JOIN-ит tags+file_tags + LEFT JOIN frontmatter_fields за progress. АЛЬТЕРНАТИВА: frontmatter-поле goal: (тогда всё в frontmatter_fields, проще запрос, НО отступление от дизайна — нужно обоснование). Рекомендация: тег #goal (как дизайн-доки).
6. D6 (Goals — шкала и маппинг): подтвердить шкалу 0–100 (источник истины PKM_Home:76 + DESIGN_BRIEF:130) и трактовку: 0≤x≤1 как доля (×100, т.е. 0.5→50%) ИЛИ как 0.5%? Также подтвердить strip trailing % («80%»→80). parser хранит сырое (progress:0.5 в тесте parser/mod.rs:454) → маппинг фиксируется на чтении. Рекомендация: 0–100 канон; 0≤x≤1 → ×100; strip %.
7. D7 (Goals — политика битых значений, no silent caps): поведение при теге #goal без валидного progress (отсутствует / >100 / нечисловое) — скрыть строку, показать без бара + бейдж «нет прогресса», или «—». Требуется ЯВНОЕ решение, не тихий 0%. Рекомендация: показать строку без бара + бейдж «нет прогресса» (полнота: цель есть, прогресс не задан) ИЛИ скрыть (меньше шума) — выбор владельца.
8. D8 (примечание, не равновесная развилка): формальный ADR-007 НЕ занимать (тройная бронь: #21-планировщик + загрузка-кода-плагина). Рекомендация (очевидная): спека без ADR; при оформлении — следующий свободный номер. Подтвердить.
9. D9 (примечание, не равновесная развилка): /template держать заглушкой до отдельной спеки классификатора (inline-llm.md §7); дешёвый ретрив уже покрыт Related Notes. Рекомендация (очевидная): заглушка. Подтвердить.

