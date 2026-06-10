# Дизайн-паритет: приложение = макет (эпик DP, решение владельца 2026-06-10)

> Владелец: «Берём в работу весь дизайн ПОЛНОСТЬЮ. Итоговый вид приложения должен быть как на
> макете». Источник истины визуала — обновлённый handoff-бандл в `docs/design/handoff/`
> (рефреш 2026-06-10: макет ранее был приведён К РЕАЛЬНОСТИ приложения по `APP_VS_MOCKUP.md`,
> теперь приложение доводится до макета). Читать вместе с README бандла; финальные решения
> владельца — в чат-транскрипте дизайн-сессии (вне репо).

## Протокол среза

Каждый DP-срез — линейный PR от main по штатной дисциплине (test-all → PR → зелёный CI → мерж)
плюс **визуальная сверка**: превью `nexus-be-web:1431` (cwd-проверка!) против соответствующего
экрана `docs/design/handoff/*.html`, обе темы, скриншоты в PR/треде. Прототипные демо-механики
(селекторы состояний, mock-данные) не переносятся — состояния выводятся из реальных данных.

## Границы

- **AI-панель (`ai-panel.jsx`, `ai.css`) НЕ трогаем** до мержа дизайн-PR #97 (рендер reasoning +
  fidelity панели уже там). После него — DP-12 (layouts side/bottom/overlay, RAG-стили,
  чат-бейдж E9 + AC-EGR-14 = egress срез 2 ч.2).
- **News (`news.jsx/.css`)** — уже в main (NF-5/6).
- `BrandThinking` глобальные классы живут в `src/motion.css` (DP-0); компонент — у первого
  потребителя (DP-1 Home), `components/chrome/`. PR #97 несёт свой в `components/chat/` —
  после его мержа свести к одному (хаускипинг-пункт DP-12).
- Демо-tweaks прототипа `platform/offline/cloud` не переносятся (платформа — реальная,
  offline — реальный egress-стейт).

## Нарезка (порядок исполнения)

| Срез | Объём | Зависимости | Статус |
|---|---|---|---|
| **DP-0** | фундамент: рефреш `docs/design/handoff/` (вкл. темы Midnight Ink / Platinum Slate в tokens), `src/motion.css` (ease-spring/out/inout, dur-1..4, brand-thinking/mt-label, reduced-motion), токены премиум-тем в `styles.css` (инертны до DP-4/11), этот план | — | ✅ #123 |
| **DP-1** | **Home-дашборд**: бэкенд H6 `get_home_activity` (heatmap/changes-today/streak/orphan по mtime-данным БД; continue = последняя заметка + сниппет) + страница `home.jsx`: greeting (serif 30, имя акцент-курсивом, live-чип провайдера), hero-search, continue-карта, quick actions, sec-label'ы, grid-2 секции (сводка: daily brief AI + recent · активность: метрики+heatmap · граф-мини · проекты: goals+stats · внимание: stale+open questions · анализ: focus drift), AI-карты (teal-кант+бейдж+thinking), reveal-анимации; side-nav вход + Home как стартовая вью | DP-0; HOME-бэкенд H1–H5 ✅ | ✅ (этот PR) |
| **DP-2** | сайдбар: icon-rail (files/search/tags/starred), side-nav (Home/Новая заметка), tree-row по макету (twist/depth-indent/★), tags-панель (+команда `list_tags`), starred (localStorage v1), search-панель | DP-1 (side-nav) | ⏳ |
| **DP-3** | редактор: floating-табы на chrome-фоне (m-tabin, dirty/close, tab-add), **DnD табов между панами** (`text/nexus-tab`, drop-target), tab-tools sticky, **mode-float пилюля** (⌘E, иконка-действие), doc-meta чипы, md-fidelity (буллеты-акцент, quote, inline-code, wikilink-скобки, нумерация), reading-mode обработка табов, backlinks-bar collapsible twist | DP-0 | ⏳ |
| **DP-4** | титлбар: **AI-dropdown** (Дайджест/Цели/Противоречия под sparkles — разгрузка бара), кнопка reading-mode, порядок по макету; статусбар: sync-дот, **прогресс-бар индексации**, conflict-пилюля, Local/UTF-8/Markdown; theme store: 4 темы + цикл | DP-0 | ⏳ |
| **DP-5** | палитра: секции **Файлы** (`search_vault`) + Команды, footer-хинты, glass + stagger, стили top/center/spotlight | DP-0 | ⏳ |
| **DP-6** | граф: подсветка current (halo+ripple+glow) и соседей, **flow-рёбра** (edge-pulse stagger), drag-pin с тёплой симуляцией, BrandThinking-лоадер, beside/fullscreen, форс-панель | DP-0 | ⏳ |
| **DP-7** | онбординг 4 шага: welcome → vault (открыть/новый/demo) → AI (health-pill через `test_ai_connection`, skip) → индексация (реальный прогресс) → вход | DP-0 | ⏳ |
| **DP-8** | плагины: карточки (glyph/имя/версия/автор/perm-чипы safe/caution/sensitive), тогглы, **consent-sheet** (risk-бейджи, Allow/Cancel, revocable-note), вкладка журнала доступа (брокер-аудит); permissions в `PluginInfo` | DP-0 | ⏳ |
| **DP-9** | инсайт-модалки по `insights.jsx`: serif-дайджест с **bold**, мета+AI-бейдж, thinking-знаки, цели-треки, contra-карточки A↔B с бейджами | DP-0 | ⏳ |
| **DP-10** | sync (цветные статусы, commit-message, secrets-баннер, remote) + conflict 3-way (hunk-бары, chosen/dimmed, правый рейл, bulk) | DP-0 | ⏳ |
| **DP-11** | настройки/tweaks: 4 темы в Appearance, density compact/comfortable/**auto** (брейкпоинт 1180), chrome standard/minimal (`--chrome`), editorFont sans/serif/mono, paletteStyle | DP-4/5 | ⏳ |
| **DP-12** | AI-панель: layouts side/bottom/overlay, RAG-стили cards/chips/footnotes, чат-бейдж local/cloud/offline (E9) + i18n `EgressDenied` (**AC-EGR-14**); унификация BrandThinking | **мерж #97** | ⛔ blocked |

## Бэкенд-добавки по ходу эпика

- **H6 (DP-1)**: `get_home_activity` — heatmap (дни × недели по `files.mtime`), changes today,
  streak (подряд дней с правками), orphan count (без беклинков), continue (последняя заметка:
  путь/заголовок/сниппет/слова). Честно из имеющихся данных; исторических счётчиков слов нет —
  не выдумывать (no silent caps → BACKLOG, если чего-то не хватает визуалу).
- **`list_tags` (DP-2)**: теги с количеством из индекса.
- **permissions в `PluginInfo` (DP-8)** из манифеста + персист consent-решений.

## Верификация эпика (выход)

Финальный проход: каждый экран приложения ↔ соответствующий экран `handoff/*.html` в 4 темах
(light/dark/midnight/platinum), скриншот-пары владельцу; расхождения — списком с решением
(фикс или фиксация отличия как осознанного).
