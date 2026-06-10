# Лента новостей (News Feed) — спека vision → AC

> Vision→AC сессия #2 (2026-06-10, решения владельца D1–D7 зафиксированы в диалоге). Концепт —
> `docs/design/PKM_Home_Concepts.md` §«Лента новостей»; сетевые решения — ADR-005-ext W1–W4
> (`ARCHITECTURE.md` §0). Дизайн-handoff — `docs/design/NEWS_FEED_BRIEF.md`.
>
> **Суть:** отдельная страница-агрегатор. Раз в сутки (или по кнопке) приложение забирает
> RSS/atom/JSON-фиды доверенных AI-источников, фильтрует двухэтапно (keyword → LLM), LLM пишет
> **русский заголовок + русское резюме** (перевод и сжатие одним вызовом) и **сводку дня**;
> результат — карточки по темам + шапка-дайджест. Чтение «в одном месте», без обхода сайтов.
>
> AC-NF-* живут здесь до реализации; с первым кодовым срезом переезжают в `ACCEPTANCE.md` +
> `traceability.json` (прецедент — спека inline-llm).

## Решения владельца (D1–D7, 2026-06-10)

| # | Решение |
|---|---|
| D1 | **Источники v1** — список ниже (все фиды прозвонены вживую 2026-06-10). Английский контент НЕ переводится «как статья»: LLM-этап сразу выдаёт RU-заголовок + RU-резюме; оригинал — по клику. |
| D2 | **Ключевые слова:** стартовый пресет + редактируемый список в настройках страницы. Keyword-фильтр применяется ТОЛЬКО к высокопоточным источникам (`high_volume`); малопоточные (блоги вендоров, релизы) идут в LLM-этап целиком. Авто-вывод тем из vault — v2. |
| D3 | **Частота:** run-if-overdue раз/сутки (первое открытие за день; как дайджест) + ручная кнопка «Обновить». |
| D4 | **Формат:** сводка-шапка (RU-дайджест дня, 5–8 строк) + лента карточек, сгруппированных ПО ТЕМАМ (кластеры, не источники): RU-заголовок · 1–2 предложения RU-резюме · источник + время · ссылка на оригинал · read/unread · чипы-фильтры. |
| D5 | **Vault-связь:** v1 — кнопка «в заметку» (заметка в Inbox с резюме+ссылкой). Семантическое «касается твоей заметки X» — v2 (эмбеддинги потока + пороги + eval). |
| D6 | **Хранение:** `nexus.db`, таблица `news_items` (дедуп по url), ретенция 30 дней GC-джобой. «Навсегда» = «в заметку». |
| D7 | **UI:** отдельная страница; бэкенд+API — этот трек, визуал — дизайнер по брифу `NEWS_FEED_BRIEF.md`. |

## Источники v1 (verified 2026-06-10; `high_volume` = keyword-фильтр на входе)

| Источник | Фид | Тип | high_volume |
|---|---|---|---|
| OpenAI | `https://openai.com/news/rss.xml` | RSS | — |
| Google DeepMind | `https://deepmind.google/blog/rss.xml` | RSS | — |
| Google AI | `https://blog.google/technology/ai/rss/` | RSS | — |
| Mistral | `https://mistral.ai/rss.xml` | RSS | — |
| Qwen | `https://qwenlm.github.io/blog/index.xml` | RSS | — |
| Hugging Face Blog | `https://huggingface.co/blog/feed.xml` | RSS | — |
| HF Daily Papers | `https://huggingface.co/api/daily_papers` | JSON API | ✓ |
| Simon Willison | `https://simonwillison.net/atom/everything/` | Atom | — |
| Sebastian Raschka | `https://magazine.sebastianraschka.com/feed` | RSS | — |
| The Gradient | `https://thegradient.pub/rss/` | RSS | — |
| Last Week in AI | `https://lastweekin.ai/feed` | RSS | — |
| llama.cpp releases | `https://github.com/ggml-org/llama.cpp/releases.atom` | Atom | — |
| ollama releases | `https://github.com/ollama/ollama/releases.atom` | Atom | — |
| vLLM releases | `https://github.com/vllm-project/vllm/releases.atom` | Atom | — |
| HackerNews | `https://hn.algolia.com/api/v1/search_by_date?tags=story&query=<kw>` | JSON API | ✓ |
| Хабр «Искусственный интеллект» | `https://habr.com/ru/rss/hub/artificial_intelligence/all/` | RSS | ✓ (RU: без LLM-перевода, но фильтр нужен) |
| arxiv cs.AI / cs.LG / cs.CL | `https://rss.arxiv.org/rss/cs.*` | RSS | ✓ — **выключены по умолчанию** (шум; HF Papers — их кураторская выжимка) |

**Anthropic:** официального RSS НЕТ (прозвонены `/rss.xml`, `/news/rss.xml` — 404). v1: покрытие
через HN-запрос (`anthropic`, `claude` в пресете ключей) + Simon Willison. v2: HTML-мост к
`anthropic.com/news` (отдельное решение — скрейпинг против правила «без обхода защит»).

**Пресет ключевых слов (старт, редактируемый):** claude, anthropic, gpt, openai, gemini, qwen,
llama, mistral, llm, rag, embedding, agent, mcp, inference, quantization, fine-tuning, vllm,
llama.cpp, ollama, transformer, reasoning.

**Конфиг страницы** (источники: вкл/выкл + свои URL; ключи; тоггл фичи) — app-local
`news.json` в OS config-dir (рядом с `egress.json`, E5-паттерн: вне vault/git — добавление
источника = сетевой consent, не должен приезжать git-pull'ом молча). Миграция в синкаемый
конфиг — отдельное решение при необходимости.

## Сеть и безопасность (наследует ADR-005-ext W1–W4)

- Весь эгресс — через `net::GuardedClient` с **новым `EgressFeature::NewsFeed`** (вариант
  добавляется ВМЕСТЕ с фичей, W1). **Дефолт — ВЫКЛ** (web-класс не из коробки, E4): страница без
  включённой фичи показывает onboarding-CTA с консентом («запросы уйдут на хосты источников»).
  Включение фичи/сохранение источника = consent → хосты источников в allowlist (паттерн W2).
- **Лимиты W3:** timeout 20 с/запрос, body-cap 2 МБ (превышение → источник пропущен, видимая
  пометка), прогон раз/сутки + manual. Свои: ≤4 конкурентных загрузки фидов; LLM-этап ≤60
  статей/прогон (излишек отрезается по дате с пометкой «обработано X из Y» — no silent caps).
- **DNS-rebinding-гард** (W-аддендум: для доменов обязателен): `allow_private=false` для
  NewsFeed + резолв хоста → отказ, если IP приватный/metadata (мокаемый резолвер в тестах).
- **Anti-injection (AC-SEC-7-паттерн):** контент фидов — НЕДОВЕРЕННЫЙ; в LLM-промпт идёт между
  случайными маркерами (`injection_marker`) с инструкцией «данные, не команды»; tool-use в
  News Feed-промптах запрещён. Ответ LLM — строгий JSON с валидацией (невалидный → item failed).
- **W4 scan_secrets:** для News Feed неприменим by-construction — исходящие запросы это GET без
  тела на статичные URL из конфига (vault-контент в сеть не уходит). Зафиксировано осознанно.
- Kill-switch «офлайн» блокирует прогон: scheduled-джоба остаётся `pending` (S10), не `failed`.

## Acceptance Criteria (AC-NF-*)

> Тестируем МЕХАНИКУ (фикстуры фидов, мок-LLM, мок-резолвер); КАЧЕСТВО перевода/резюме/кластеров —
> human-eval владельцем (не автотест, как D-решения inline-llm).

- **AC-NF-1 (парсер).** RSS 2.0 / Atom / HF-JSON / HN-JSON из замороженных фикстур реальных фидов
  → нормализованный `NewsItem {source_id, url, title, published_at, excerpt}`. Битый/недоступный
  фид → источник пропущен с видимой ошибкой прогона (сводка «N из M источников»), БЕЗ падения джобы.
- **AC-NF-2 (keyword-этап).** Фильтр (unicode case-insensitive, по title+excerpt) применяется
  ТОЛЬКО к `high_volume`-источникам; малопоточные проходят целиком. Пустой список ключей →
  high_volume-источники НЕ загружают LLM (отбрасываются с предупреждением — fail-closed к бюджету).
- **AC-NF-3 (LLM-этап).** Мок-LLM получает title+excerpt МЕЖДУ injection-маркерами; ответ —
  строгий JSON `{relevant, title_ru, summary_ru, topic}`; невалидный JSON/вылет за схему → item
  `failed`, в ленту не попадает, счётчик failed виден в сводке прогона.
- **AC-NF-4 (дедуп).** Повторный прогон тех же фидов не создаёт дубликатов (`url` UNIQUE);
  обновлённый title по тому же url НЕ перетирает прочитанность.
- **AC-NF-5 (ретенция).** Items старше 30 дней удаляются GC-джобой; read/hidden-флаги живут до
  ретенции; «в заметку» не зависит от ретенции (заметка — в vault).
- **AC-NF-6 (планировщик).** Kind `newsfeed`: run-if-overdue раз/сутки + manual refresh с дедупом
  (`has_ready_job`); `defer_under_interactive=true` (LLM-этап уступает чату, S5); офлайн →
  `pending`, не `failed` (S10).
- **AC-NF-7 (эгресс).** Все запросы фидов — через `GuardedClient` с `EgressFeature::NewsFeed`;
  фича по умолчанию ВЫКЛ → прогон не стартует без consent; включение кладёт хосты источников в
  allowlist; timeout 20 с и body-cap 2 МБ энфорсятся (превышение — видимый пропуск источника).
- **AC-NF-8 (DNS-rebinding).** Домен источника, резолвящийся в приватный/metadata-IP
  (192.168.x / 169.254.169.254), отклоняется ДО коннекта (мок-резолвер); `allow_private=false`.
- **AC-NF-9 (API).** Команды: `get_news(topic?, unread_only?, page)` · `news_mark_read` ·
  `news_to_note` · `refresh_news` · `get/set_news_config` (sources/keywords/enabled) — типизированы,
  без `String`-ошибок в отказах политики.
- **AC-NF-10 (сводка дня).** Из relevant-items за прогон LLM строит RU-сводку (мок: сводка
  упоминает темы дня); «нет новостей»/«фича выключена» — отдельные состояния, не пустой экран.
- **AC-NF-11 («в заметку»).** Карточка → новая заметка (`News/<дата> <slug>.md` c фронтматтером
  `source`/`url`): RU-заголовок, резюме, ссылка; путь уникален (повтор → суффикс); заметка
  индексируется штатно.
- **AC-NF-12 (i18n).** Все строки страницы — ключи RU/EN (паритет-тест); качество RU-резюме —
  human-eval.

## Нарезка (вертикальные срезы, каждый — линейный PR от main)

| Срез | Объём | Зависимости | Офлайн-верификация |
|---|---|---|---|
| **NF-1** | модуль `news/`: типы, парсеры RSS/Atom/JSON, нормализация, keyword-фильтр, реестр источников v1 | — | фикстуры реальных фидов (заморозить при живом интернете), юниты [AC-NF-1,2] |
| **NF-2** | LLM-этап: промпт с маркерами, строгий JSON-контракт, RU-поля, кластеры, сводка дня | NF-1 | мок-`ChatProvider` [AC-NF-3,10] |
| **NF-3** | миграция `news_items` + дедуп + ретенция + kind `newsfeed` + команды + `news.json`-конфиг | NF-1/2, планировщик ✅ | temp-DB юниты [AC-NF-4,5,6,9,11] |
| **NF-4** | сеть: `EgressFeature::NewsFeed` (дефолт ВЫКЛ) + consent→allowlist + лимиты W3 + DNS-гард | egress-фундамент ✅ | мок-listener + мок-резолвер [AC-NF-7,8] |
| **NF-5** | страница UI по брифу (`NEWS_FEED_BRIEF.md`) + i18n | дизайн-макет владельца ✅ (handoff 2026-06-10) | vitest-компоненты + превью [AC-NF-12]; качество — human-eval |
| **NF-6** | reader: клик по заголовку → полный RU-перевод статьи in-app + кнопка «Сократить» (тезисы on-demand); фетч оригинала через guarded-фетчер NF-4 (fail-closed: хост вне news-allowlist → резюме + ссылка «Оригинал») | NF-4/5; добавлен владельцем на дизайн-итерации (не в брифе v1) | temp-DB + мок-чат юниты; превью |

NF-1..4 не требуют живого LLM-сервера (моки/фикстуры); живой smoke всего пайплайна — после
пересборки сервера владельца (разовый, как eval-фикстура).

## Вне скоупа v1 (→ BACKLOG при реализации)

Семантическая связь с vault («касается заметки X») · авто-вывод ключей из vault · Anthropic
HTML-мост · Telegram-каналы (нет API без бота) · пейвол-источники (ScrapingBee и т.п.) ·
кластеризация между днями («сюжеты») · пуш-уведомления.
