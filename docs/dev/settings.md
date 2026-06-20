# Раздел настроек (кросс-план #11)

Полноэкранная модалка настроек по образцу Obsidian: левый нав секций + контент-панель. Заменила
разрозненную панель «Оформление» (бывшая `TweaksPanel`). Собирается слайсами.

- **UI:** `apps/desktop/src/components/settings/SettingsView.tsx` (+ `.module.css`, `.test.tsx`).
- **Состояние:** `stores/ui.ts` — `settingsSection: 'appearance'|'ai'|'hotkeys'|'about'`,
  `setSettingsSection`, `openSettings(section?)`. Открытие — флаг `tweaksOpen` (исторический; та же
  модалка), команда `view.settings` (**Cmd/Ctrl+,**) + шестерёнка в титлбаре.
- **Бэкенд AI-секции:** `apps/desktop/src-tauri/src/commands/settings.rs`.

## Секции

| Секция | Статус | Что делает |
|---|---|---|
| Основное | ✅ слайс 3 | Язык RU/EN (`changeLocale`; дублирует быстрый тогл в титлбаре). |
| Редактор | ✅ слайс 3 | «Читаемая ширина строки» → CSS-переменная `--editor-max-width` (`stores/prefs`). |
| Оформление | ✅ слайс 1 | Тема (светлая/тёмная), акцент, плотность — через `stores/theme`. |
| AI / Модели | ✅ слайс 2 | URL/модель chat- и embedding-эндпоинтов, «Проверить связь», «Сохранить». |
| Горячие клавиши | ✅ слайс 4 | Список команд + захват комбинации, сброс к дефолту, подсветка конфликтов. |
| О программе | ✅ слайс 1 | Имя, версия (`app_version`), путь vault. |

**«Читаемая ширина строки» (Редактор).** Тема редактора (`components/editor/extensions.ts`,
`.cm-content`) читает `max-width: var(--editor-max-width, none)` + `margin-inline: auto`. Тогл в
`stores/prefs.ts` выставляет переменную на `<html>` (`44rem` ВКЛ / `none` ВЫКЛ), персист в localStorage,
применение на старте (импорт в `main.tsx`). Реактивно через CSS-каскад — **без** пересоздания `EditorView`.

## AI / Модели — контракт команд

Форма читает/пишет `.nexus/local.json` (вне git, ADR-002) **из UI** — без ручного редактирования
файла. Три IPC-команды (фронт ходит через `tauriApi.settings.*`, контракт §4.1):

- **`get_ai_config() -> AiConfigDto`** — префилл формы: парсит `local.json`, возвращает
  `{ chat?: {url, model?}, embedding?: {url, model?} }`. Нет файла/пустой → пустой конфиг.
- **`set_ai_config(chat?, embedding?) -> { chatApplied, embeddingChanged }`** — мерж в `local.json`
  через `serde_json::Value`, **сохраняя прочие ключи** (`sync` и т.п.); затем горячее применение chat.
  Чистый мерж вынесен в `apply_ai(doc, chat, embedding)` (тестируется без `State`).
- **`test_ai_connection(url)`** — пробный `GET {url}/v1/models` (OpenAI-совместимо). Любой ответ
  сервера → достижим; сетевая ошибка → `Err`. Через `core_client_builder` (**redirect=none**, анти-SSRF,
  AC-SEC-4) с таймаутом 5 c.

### Горячее применение vs перезапуск

- **Chat применяется немедленно.** `OpenAiChatProvider` stateless per-request — команда `chat_rag`
  читает `ctx.chat` из state на каждый запрос. `set_ai_config` просто подменяет `ctx.chat` под
  `vault.write()`. Никаких фоновых задач на нём не висит → безопасно.
- **Embedding требует перезапуска.** На embedding-провайдере висит фоновый индексатор (свой клон
  `Arc<dyn EmbeddingProvider>` + общий `vectors`), который пишет векторы старой моделью. Безопасный
  in-place hot-swap потребовал бы остановки/респавна индексатора и переиндексации — отдельный объём
  (#11b-full). Поэтому при изменении `embedding` команда возвращает `embeddingChanged: true`, а UI
  показывает «перезапустите приложение». На переоткрытии vault конфиг перечитывается (`#8
  load_local_config`) и индексатор стартует с новой моделью.

## Поток данных

```
SettingsView/AiSection
  └─ tauriApi.settings.{getAiConfig,setAiConfig,testConnection}
       └─ (Tauri) commands::settings::*          (вне Tauri → lib/mock/settings.ts)
            ├─ get/set: .nexus/local.json  (serde_json::Value, прочие ключи сохранены)
            └─ set: ctx.chat = OpenAiChatProvider::new(url, model)   ← горячо
```

## Горячие клавиши — движок ремапа

Реестр команд (`lib/commands.ts`) — единственный источник истины (ядро/плагины/пользователь). Слайс 4
добавил поверх него:

- **Персист**: пользовательский ремап (combo → id) хранится в localStorage `nexus.hotkeys.v1`,
  грузится в конструкторе реестра; `setUserKey`/`remap`/`resetKey` пишут сразу.
- **`effectiveKey(id)`** — combo для UI: пользовательский оверрайд → дефолт (нормализованные).
  **`userKeyFor(id)`** — обратный поиск (есть ли оверрайд). **`remap(id, combo)`** снимает прежний
  бинд команды и ставит новый. **`resetKey(id)`** убирает оверрайд (возврат к дефолту).
- **Резолв** хоткея (`resolve`) даёт приоритет **пользователь > плагин > ядро**; конфликты не
  блокируются (UI подсвечивает: одна комбинация у ≥2 команд).
- **Захват** (`SettingsView` → `HotkeysSection`): слушатель на **capture-фазе** `window` —
  срабатывает раньше глобального `useKeymap` (`hooks/useKeymap.ts`), чтобы нажатие не выполнило команду.
  Esc — отмена; требуется модификатор (как и `useKeymap`, который игнорит ввод без модификатора).

## Тесты

- **Rust** (`commands/settings.rs#tests`): `apply_ai` — мерж задаёт `ai.chat`/`ai.embedding`, сохраняет
  посторонние ключи (`sync`), детектит смену embedding (`embeddingChanged`), `chat=None` удаляет ключ.
- **Hotkeys** (`commands.test.ts`): `remap`/`effectiveKey`/`userKeyFor`/`resetKey` + персист в
  localStorage; (`SettingsView.test.tsx`): список команд, захват комбинации (dispatch keydown), сброс.
- **Фронт** (`SettingsView.test.tsx`): рендер формы (2 эндпоинта), «Проверить связь» → бейдж «Доступен»,
  «Сохранить» → подтверждение; пустой URL → «Недоступен», смена embedding → требование перезапуска.
  Вне Tauri бэкенд проксируется в `lib/mock/settings.ts` (in-memory happy-path).

## Инференс через конфиг (INFER-CFG) — смена движка = смена `local.json`

Слой инференса **engine-agnostic**: переход llama.cpp → 1Cat-vLLM (Qwen3.6-27B-AWQ на V100) → любой
OpenAI-совместимый сервер — это правка `.nexus/local.json`, без кода. Все таймауты/сэмплинг — опц. поля
с дефолтами (`ChatConfig`/`EmbeddingConfig`-геттеры в `nexus-core/src/ai/config.rs`); zero-config работает
как раньше, только с безопасными дефолтами.

**Cold-start V100.** Первый запрос к крупной модели на V100 компилирует ядра 1–3 мин. Поэтому таймаут
расщеплён: `first_token_timeout_secs` (дефолт **300**) действует на инициацию + чанки **до первого байта**
(переживает прогрев), затем `idle_timeout_secs` (дефолт **90**) — на разрывы между чанками в steady-state.
`connect_timeout_secs` (дефолт 30) — у guarded-клиента; embedding-`timeout_secs` (дефолт 60). Контекст-окно
НЕ хардкодится (`context_window`; при отсутствии — консервативные 32K с warn, никогда 256K по умолчанию).

**Поля `ai.chat`/`ai.fast`** (`ChatConfig`): `url`, `model`, `context_window`, `first_token_timeout_secs`,
`idle_timeout_secs`, `connect_timeout_secs`, `retry_attempts`, `temperature`, `reserve_output_tokens`.
**Поля `ai.embedding`**: `url`, `model`, `dim`, `timeout_secs`. **`ai.tokenizer_path`** — токенайзер для
бюджета контекста (при отсутствии — встроенный Qwen3.6-27B). Применение: headless `nexus-agentd` и desktop
(`build_chat`/`build_util_chat`) берут весь профиль; хот-апплай настроек (`set_ai_config`) — из сохранённого
`ai.chat` сразу, кастомные стрим-таймауты иначе вступают при переоткрытии vault.

**Пример свапа на целевой сервер** (плейсхолдер `<vllm-host>`):

```jsonc
{
  "ai": {
    "chat": {
      "url": "http://<vllm-host>:8000",
      "model": "qwen3.6-27b-awq-mtp",   // --served-model-name
      "context_window": 262144,          // 256K (на 16GB-картах поставить меньше)
      "first_token_timeout_secs": 300,   // cold-start V100 1–3 мин
      "idle_timeout_secs": 90,
      "connect_timeout_secs": 30,
      "retry_attempts": 3,
      "temperature": 0.3
    },
    "embedding": { "url": "http://<embed-host>:8001", "model": "bge-m3", "dim": 1024, "timeout_secs": 120 },
    "tokenizer_path": "/vault/.nexus/tokenizer.json"  // опц.: токенайзер целевой модели
  }
}
```

Запуск целевого сервера (1Cat-vLLM): `--enable-auto-tool-choice --tool-call-parser qwen3_coder`
(OpenAI tool-calling, совместимо с провайдером Nexus). Известный риск MTP-коллапса — пинить релиз `0.0.3`
или MTP-off (см. память `project_target_llm_server_1cat_vllm`).
