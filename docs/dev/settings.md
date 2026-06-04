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
| Горячие клавиши | ⏳ слайс 4 | Переназначение keymap (пока заглушка). |
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

## Тесты

- **Rust** (`commands/settings.rs#tests`): `apply_ai` — мерж задаёт `ai.chat`/`ai.embedding`, сохраняет
  посторонние ключи (`sync`), детектит смену embedding (`embeddingChanged`), `chat=None` удаляет ключ.
- **Фронт** (`SettingsView.test.tsx`): рендер формы (2 эндпоинта), «Проверить связь» → бейдж «Доступен»,
  «Сохранить» → подтверждение; пустой URL → «Недоступен», смена embedding → требование перезапуска.
  Вне Tauri бэкенд проксируется в `lib/mock/settings.ts` (in-memory happy-path).
