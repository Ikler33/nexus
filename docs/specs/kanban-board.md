## Спецификация эпика «Канбан/задачи с AI» (decision-complete)

### 1. Модель данных «задача = заметка»
Frontmatter-схема (всё — плоские top-level скаляры → уже в `frontmatter_fields`, кроме `tags`-списка → `file_tags`):
```yaml
---
status: todo          # ОБЯЗАТЕЛЬНО — наличие ключа = заметка является задачей
project: Nexus        # опц. строка — группировка/свимлейн/фильтр
priority: high        # опц. low|medium|high|urgent
due: 2026-06-20       # опц. ISO YYYY-MM-DD — дедлайн-бейдж/overdue
created: 2026-06-16   # опц. — для stale-радара (A2)
tags: [task, frontend]# опц. — уже идёт в file_tags
---
# Тело = описание + чеклисты + [[ссылки]]
```
Нормализация (детерминированная): `trim`, снятие кавычек (уже делает `frontmatter_fields`), сравнение колонок case-insensitive по lowercase, отображение — как ввёл пользователь. `due` парсится лениво (валидная ISO → бейдж, иначе игнор). Минимальный frontmatter при создании: только `status` (= первая колонка), прочее — по мере задания.

### 2. Доска
- **Доска = вью**, не сущность. Колонка = значение `statusKey` (по умолч. `status`).
- Виртуальная колонка **«Прочее»** в конце для значений вне набора колонок (не теряем задачи).
- «На доску» из контекстного меню заметки без `status` = `set_frontmatter_field(path,'status',<первая колонка>)`.
- Конфиг колонки: `{id,label,wip,color,doneLike}` — `id` = raw-значение в `status` (источник истины), `label` = отображение (переименование без правки файлов), `doneLike` = терминальная колонка.

### 3. Персист (board JSON `.nexus/boards/<slug>.json`, синк с vault)
```json
{ "id":"personal","title":"Личные задачи","statusKey":"status",
  "columns":[{"id":"todo","label":"К выполнению","wip":null,"color":"slate","doneLike":false}, ...],
  "scope":{"folder":null,"project":null,"tags":[]},
  "swimlanes":"none","order":{"todo":["Notes/a.md"]},
  "sort":"manual","cardFields":["due","priority","tags"] }
```
Глобальные дефолты (набор колонок, statusKey, дефолтная доска) — `.nexus/config.json`. Запись — `atomic_write_io`, best-effort. Список досок = `ls .nexus/boards/`. Это и есть «разбивка по проектам» на уровне вью (доска «Работа» scope `project:Nexus`, доска «Дом» scope `folder:Home/`).

### 4. Drag-n-drop (ключевая механика)
DnD карточки между колонками → optimistic UI → `set_frontmatter_field(path,'status',targetColId)` → новый hash в `Buffer.baseHash` (анти-эхо SAFE-3) → при ошибке откат + тост. Реордер пишет только board JSON (1 файл, атомарно), не frontmatter (нет фантомных `edit_events`). Битый YAML → `MalformedFrontmatter`, карточка не двигается, файл не перезаписан. Watcher `vault:changed` → живой пересчёт доски.

### 5. Индексация (без новых таблиц)
Выборка доски — клон `goals::list_goals`:
```sql
SELECT f.path,f.title,st.value AS status,pr.value AS project,pri.value AS priority,due.value AS due
FROM files f
JOIN frontmatter_fields st ON st.file_id=f.id AND st.key='status'      -- INNER = только задачи
LEFT JOIN frontmatter_fields pr ON pr.file_id=f.id AND pr.key='project'
LEFT JOIN frontmatter_fields pri ON pri.file_id=f.id AND pri.key='priority'
LEFT JOIN frontmatter_fields due ON due.file_id=f.id AND due.key='due'
WHERE f.is_deleted=0   -- + опц. scope: path LIKE / JOIN file_tags / project-фильтр
```
`idx_frontmatter_fields_key` уже есть. БД — rebuildable-кэш (реиндекс восстанавливает).

### 6. Write-back (`set_frontmatter_field`, новый модуль `tasks/frontmatter_edit.rs`)
Алгоритм: `split_frontmatter` → (a) нет FM → создать `---\nkey: value\n---\n` перед телом; (b) top-level строка `^key:\s` (тот же критерий, что `frontmatter_fields`: без ведущих пробелов/`-`) → заменить значение, прочее байт-в-байт; (c) ключа нет → дописать перед закрывающим `---`. Квотирование `value` при `: # [ { - ? ,` / ведущем-хвостовом пробеле / text-похожем-на-число-bool-date; `[[ссылки]]` в кавычках. Списки — блок-стилем. `atomic_write` → blake3-hash. Снапшот истории best-effort. Идемпотентность `parse∘write∘parse` (property-тест). Битый FM → `Err(MalformedFrontmatter)`, файл не трогаем.

### 7. Properties-панель (Obsidian-паритет)
- Реестр `.nexus/property-types.json`: `{propName: text|list|number|checkbox|date|datetime|tags}` — тип ГЛОБАЛЕН по имени, только явно заданные; остальное — эвристика (bool→checkbox, ISO-datetime→datetime, ISO-date→date, число→number, список→list, иначе→text; `tags/aliases/cssclasses` форсятся).
- **MVP-виджеты:** text/list(чипы+autocomplete)/number/checkbox/date. datetime + спец-tags — вторым срезом.
- «Добавить свойство» (Cmd/Ctrl+;), новый ключ = text. Смена типа = иконка слева, меняет ГЛОБАЛЬНО. Запись — через `set_frontmatter_field` (единая точка).
- **Invalid:** значение не под типом → жёлтое поле, виджет заблокирован, «Править в source». Без авто-конверта (паритет).

### 8. Теги (Obsidian-паритет)
- Unicode/кириллица: `is_ascii_alphabetic`→`char::is_alphabetic` (для русскоязычного владельца — критично).
- Автокомплит инлайн `#tag` (`tagSource` по образцу `wikilinkSource` + `list_tags`, вложенность `#a/b`) + автокомплит в frontmatter `tags:`-List (где Obsidian слабее — превосходим).
- Тег-пейн: дерево вложенных тегов + частоты, клик по родителю включает детей (`tag:inbox⊇inbox/*`).

### 9. Превью задачи
Side-panel/peek по клику (`MarkdownPreview` тела + Properties-сводка с инлайн-правкой через `set_frontmatter_field`) + full-page по запросу. Не модалка-трап (видно доску одновременно).

### 10. AI-набор
**Тиры:** `chat_util` (Qwen3-4B :8084) — классификаторы/JSON; `chat_fast` (gemma без CoT) — саммари; `chat` (gemma reasoning) — декомпозиция; embedder bge-m3 — RAG. Паттерны: JSON-batch-классификатор со строгим контрактом и `failed`-учётом (`news/llm.rs`), injection-fencing (`injection_marker`, тело задачи = недоверенные ДАННЫЕ), eval-харнесс + baseline-гейт (`eval/mod.rs`).

**MVP (без vision-блокеров):**
- **A1** заметка/Inbox→задача (без LLM, контракт frontmatter).
- **A2** «застрявшие» (SQL по `edit_events` + status, порог N дней).
- **A3** план дня — отбор (детерминированная раскладка today/overdue/priority).
- **A4** авто-тег — closed-vocabulary classifier (`chat_util`, allowed_tags=теги vault, тег вне словаря = hard-fail, suggested_new ВЫКЛ; golden + baseline-гейт, precision≥0.8).

**MVP+1 (первая vision-AC-сессия, под гардами):**
- **B1** разбить на подзадачи (`chat`/`chat_fast`; структурный hard-gate 3≤N≤12/длина/дубли/язык/markdown + LLM-judge {relevance/coverage/non-hallucination}≥3.5/5, 0×score1).
- **B3** саммари доски/проекта (`chat_fast`; числовой hard-gate: счётчики статусов = ground-truth regex + faithfulness≥0.9).

**Отложено (требуют vision→AC):** B2 приоритет (top-1 agr≥0.6, субъективно), B4 «почему застряла», B5 estimate (дискр. S/M/L/XL, off-by-one≤1 в ≥80%), B6 план-дня-текст, B7 авто-статус (owner-gated мутация, done-precision строгая), A5 авто-проект (accuracy≥0.75), suggested_new-теги. Все vision-фичи несут «КАК ИЗМЕРИТЬ» (порог+правило); golden рядом с `eval/golden.json`, baseline-регресс-гейт (AC-EVAL-3).

### 11. Нарезка слайсов (исполнимая)
BOARD-1 (write-back) → BOARD-2 (list_board) → BOARD-3 (board-конфиг/order-GC) → BOARD-4 (top-level вью) → BOARD-5 (DnD) → BOARD-6 (превью); PROP-1 (Unicode-теги/обзор ключей) → PROP-2 (реестр типов) → PROP-3 (Properties-панель) → PROP-4 (автокомплит/тег-пейн); AI-1 (A1) → AI-2 (A2+A3+A4) → AI-3 (B1+B3 под гардами). Зависимости: BOARD-1 — фундамент всего write-пути (DnD+Properties); PROP-3 зависит от BOARD-1+PROP-2; AI-1 зависит от BOARD-1. Детали scope/тестов/AC — в массиве slices.

### 12. Граничные случаи (для тестов без владельца)
| Случай | Поведение |
|--------|-----------|
| Заметка без `status` | Не задача; «На доску» = set status первой колонкой |
| `status` вне колонок | Виртуальная колонка «Прочее» |
| Битый YAML | `MalformedFrontmatter`, карточка не двигается, файл цел |
| Файл не в `order` | Дописывается в конец (сорт due→path), order самозалечивается |
| Удалён файл | GC из `order` при чтении доски |
| Rename/move задачи | Путь в board JSON обновляется в той же операции (CURATE-2 хук) |
| Переименование проекта | Явная batch `set_frontmatter_field` по всем `project:X`, прогресс+отчёт |
| Эхо-сейв после DnD | Новый hash → `baseHash` → guard SAFE-3 не срабатывает |
| Реордер | Запись только в board JSON → нет фантомных `edit_events` |
| Мульти-проект `[a,b]` | v1 не поддержан (не скаляр) → BACKLOG: таблица `file_projects` |
| Кириллица-тег `#тег` | Валиден после is_alphabetic (обновить зафиксированный тест) |
| Invalid-свойство | Жёлтое поле, виджет заблокирован, escape в source |

### 13. Релевантные файлы Nexus (абсолютные пути)
- Парсер frontmatter (расширять + write-back): `/Users/artem/Documents/NEXUS-be/apps/desktop/src-tauri/src/parser/mod.rs` (`split_frontmatter`:209, `frontmatter_fields`:347, теги:`push_tag`)
- Эталон выборки: `/Users/artem/Documents/NEXUS-be/apps/desktop/src-tauri/src/goals/mod.rs` (`list_goals`)
- Индекс полей (готов, новых миграций нет): `.../src/db/migrations/003_frontmatter_fields.sql`; следующая свободная — `018_*.sql` (если решат вынести типы/доски в SQL)
- Атомарная запись: `.../src/vault/mod.rs` (`atomic_write`:128, `atomic_write_io`); образец команды записи + baseHash: `.../src/commands/vault.rs` (`write_file`:712)
- Индексатор (frontmatter_fields пишутся): `.../src/indexer/mod.rs` (:260)
- Теги: `.../src/tags.rs` (`list_tags`/`notes_by_tag`); инлайн-теги/автокомплит: `.../src/components/editor/extensions.ts` (`wikilinkSource`:115 — образец для tagSource)
- Top-level вью: `.../src/components/chrome/ActivityBar.tsx`, `.../src/stores/ui.ts` (`openNews`:269), `.../src/App.tsx`:246/218/100
- Превью: `.../src/components/editor/MarkdownPreview.tsx`
- AI: `.../src/ai/mod.rs` (AIClient тиры), `.../src/ai/chat.rs` (`injection_marker`/`build_rag_messages`/`parse_web_query_plan`), `.../src/news/llm.rs` (JSON-batch-классификатор — шаблон A4), `.../src/eval/mod.rs` + `.../eval/golden.json` (eval-гейт), `.../src/scheduler/mod.rs` (фоновые AI-джобы)
---

## 14. Поправки adversarial-ревью (ОБЯЗАТЕЛЬНЫ — фолд 3 рецензентов, 2× needs-changes)

Три рецензента (данные/выполнимость · AI-измеримость/Obsidian · scope/UX). Ниже — биндинг-дельты к §1–13; при расхождении приоритет у этого раздела.

### 14.1 Корректный порядок MVP-слайсов (пересортирован по независимой мержабельности и preview-верифицируемости)
1. **PROP-1** — Unicode/кириллица-теги, СТЭНДАЛОН первым (owner-critical: владелец русскоязычный, `#тег` сейчас режется). Мелкий парсер-фикс + обновление зафиксированного фикстур-теста.
2. **BOARD-1 + промоут** — `set_frontmatter_field` СО своим call-site: команда «На доску»/промоут (из AI-1) складывается СЮДА, чтобы бэкенд-примитив был верифицируем на мерже (а не «чистый бэкенд без потребителя»).
3. **BOARD-2 + BOARD-4** — `list_board` (SQL) + top-level вью «Доска» вместе (вью без данных не проверить).
4. **BOARD-3** — board-конфиг `.nexus/boards/*.json` + order-GC + rename-хук.
5. **BOARD-5** — DnD (полный набор fail-AC, см. 14.6).
6. **BOARD-6** — превью задачи (side-panel).
7. **PROP-2 → PROP-3 → PROP-4** — реестр типов → Properties-панель → автокомплит тегов (тег-пейн-дерево вынесено, см. 14.5).
8. **AI-1** (если не сложен в BOARD-1) → **AI-2a** (застрявшие, SQL) → **AI-2b** (план-дня, детерминир.) → **EVAL-AI** (нулевой слайс, см. 14.3) → **AI-2c** (авто-тег closed-vocab).
9. **MVP+1 (vision-AC-сессия с владельцем):** AI-3 (B1/B3) — ВНЕ критического пути, НИКОГДА не блокирует BOARD/PROP/AI-1/2.

### 14.2 Сосуществование ДВУХ моделей задач (MAJOR — было не разрешено)
В коде УЖЕ есть `commands/tasks.rs`/`TasksPanel` — чеклист-строки `- [ ]`/`- [x]` (скан тел). Доска — ДРУГАЯ модель (задача=заметка-со-status). **Решение:** модели СОСУЩЕСТВУЮТ и не путаются: чеклист = «подзадачи ВНУТРИ заметки» (остаётся как есть), доска = «заметки-задачи». Доска **НЕ** использует `list_tasks`-скан — только `list_board` (SQL по индексу). В ADR/риски записать явно; в UI не смешивать (TasksPanel и BoardView — разные разделы).

### 14.3 EVAL-AI — нулевой слайс ПЕРЕД AI-2c (MAJOR)
Eval-харнесс (`eval/mod.rs`) — ТОЛЬКО RAG-retrieval (recall@k/nDCG); judge/precision/recall для классификации НЕТ. До любого LLM-классификатора (A4) и judge (B1/B3):
- расширить golden-схему на classification-кейсы; отдельный `apps/desktop/src-tauri/eval/ai_golden.json` (не мешать с retrieval-`golden.json`, подключение `include_str!`);
- детерминированный precision/recall на closed-vocab БЕЗ LLM (фиктивный классификатор на фикстуре) — чтобы гейт сам по себе тестируем;
- **A4-AC: precision≥0.8 И recall≥0.5 (или F1≥порог)** + НЕПУСТЫЕ ожидаемые наборы в golden — иначе «всегда пусто» тривиально проходит precision (класс «зелёный тест на мусоре»).

### 14.4 Write-back (BOARD-1) — усиления (MAJOR×3 + minor)
- **Матчинг ключа = ТОЧНАЯ копия логики `frontmatter_fields`** (не `^key:\s` regex): skip если строка начинается с `' '`/`\t`/`-`; `split_once(':')`; `key.trim()==target`. Не реderive-ить.
- **Квотирование — полный список** (иначе data-loss): спецтриггеры `: # [ ] { } - ? , @ \` & * ! | > % " '`, ведущий/хвостовой пробел, и значения-резерв `{null,Null,NULL,~,true,True,TRUE,false,False,FALSE,yes,Yes,no,No,on,On,off,Off}` и «похоже-на-число/дату-но-text». Property-based тест идемпотентности `parse∘write∘parse` + round-trip ЧЕРЕЗ `frontmatter_fields` (он безусловно снимает кавычки — тест на значение с `:` `[` `#` обязателен).
- **Echo-guard (SAFE-3):** команда возвращает НЕ только hash, а новый ПОЛНЫЙ контент (или фронт перечитывает файл); если заметка открыта в буфере — обновить `Buffer.baseHash` синхронно ДО watcher-события, иначе guard внешнего изменения + эхо-сейв.
- **Битый YAML** → `MalformedFrontmatter`, файл НЕ перезаписан, тост «откройте заметку».
- **Промоут** ставит status = первой НЕ-doneLike колонки (или явный `startColumn`), не `columns[0]`.

### 14.5 Properties/теги — паритет (MAJOR + minor)
- **Кириллица-тег — фикс в ТРЁХ местах** (`parser/mod.rs`): `push_tag` (~:333), инлайн-скан тела (~:186–199, перейти на `char_indices()`), и сам **`is_tag_char` как char-предикат** `c.is_alphanumeric() || matches!(c,'_'|'-'|'/')` (именно НАБОР символов режет кириллицу, не только проверка «есть буква»). Прогнать ВЕСЬ тег-тестовый набор + граф-теги + тег-пейн; не сломать ASCII-путь.
- **CM6-автокомплит:** в `extensions.ts` ЕДИНЫЙ `autocompletion({override:[wikilinkSource, slashSource]})` (явный коммент-запрет двух инстансов) → добавить `tagSource` В ЭТОТ массив; regex-контекст: `#` НЕ в начале строки-с-пробелом (исключить заголовки), не в code-span; frontmatter `tags:`-автокомплит — отдельная ветка контекста.
- **PROP-4 урезан:** оставить инлайн+frontmatter автокомплит (плоский `list_tags`). **Тег-пейн с деревом вложенности/частотами → отдельный PROP-5 / BACKLOG** (иерархическая агрегация, не в kanban-MVP).
- **Round-trip edge:** `[`/`{`-начальные и multiline `|`/`>` → помечать **invalid-полем** (не молча терять); `frontmatter_fields` их отбрасывает — Properties показывает «править в source».
- **Мульти-проект `[a,b]`** → бейдж «мульти-проект (не поддержан)» на карточке (честная деградация), не тихое выпадение.

### 14.6 UX/scope (MAJOR×3 + minor)
- **i18n в КАЖДОМ фронт-слайсе** (мандат, `i18n.test.ts` проверяет паритет ключей): `board.*`/`prop.*`/`tags.*`/`ai.*` в ОБА `en.json`+`ru.json`. Входит в acceptance каждого фронт-среза.
- **Матрица пустых/ошибочных состояний доски** (по планке NewsView) в BOARD-4/5: первый запуск, пустая доска (иллюстрация+CTA «создать задачу»), ошибка запроса → последняя валидная доска + тост, MalformedFrontmatter при DnD, конфликт. Темы/i18n.
- **BOARD-5 (DnD) — явные fail-AC:** (a) команда-reject → карточка возвращается на ТОЧНЫЙ исходный индекс, тост, без осиротевшей order-записи; (b) `baseHash` обновлён синхронно из ответа команды (анти-эхо); (c) MalformedFrontmatter → карточка не двигается; (d) optimistic-апдейт с откатом. Adversarial-ревью диффа BOARD-5 ОБЯЗАТЕЛЕН (стейт-машина).
- **AI-2 расщеплён** на AI-2a (застрявшие, SQL/edit_events — детерминир.), AI-2b (план-дня — детерминир.), AI-2c (авто-тег — LLM/golden/eval). Только 2c гейтится eval.
- **BOARD-3 rename→order** — явный под-пункт с тестом: подписка не только на UI-rename (CURATE-2), но и на watcher rename-пару (`Renamed{from,to}` by file-id); rename заметки в середине колонки → позиция сохранена, не сброшена в конец.
- **.nexus невидим watcher'у** → инвалидация доски/типов на фокус окна / явный refresh; невалидный board JSON → fallback на дефолт + тост (симметрично MalformedFrontmatter).

### 14.7 Adversarial-ревью диффа ПЕРЕД мержем (стендинг-мандат)
Обязателен после: **BOARD-1** (квотирование/echo-guard/round-trip), **BOARD-5** (DnD-стейт-машина/гонки watcher), **PROP-3** (Properties write-path), **AI-2c/AI-3** (eval-контракт/injection). Сверять вид Properties-панели и доски со скринами владельца ДО постройки (confirm-UI-before-building).
