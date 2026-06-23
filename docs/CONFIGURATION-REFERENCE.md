# Nexus — Configuration Reference

> Статус: v0.1 (2026-06-23). Полная схема конфигурации Nexus, выведенная из кода (`ai/config.rs` и др.). Источник истины — структуры Rust; этот документ их зеркалит. **Все фичи выключены по умолчанию (default-OFF), все файлы конфигурации fail-safe** (отсутствует/битый → разумные дефолты, не падение).

## Обзор файлов

| Файл | Где лежит | В git? | Поведение загрузки |
|---|---|---|---|
| **`local.json`** | `<vault>/.nexus/local.json` | gitignored (ADR-002) | per-vault; читается при открытии vault; частичный/отсутствует → дефолты |
| **`egress.json`** | `<OS-config-dir>/app.nexus.desktop/` | вне vault | kill-switch сети; битый/нет → дефолты |
| **`agent.json`** | `<OS-config-dir>/app.nexus.desktop/` | вне vault | пауза агента (kill-switch); битый/нет → unpause |
| **`websearch.json`** | `<OS-config-dir>/app.nexus.desktop/` | вне vault | согласие на веб (десктоп); нет → выключено |
| **`news.json`** | `<OS-config-dir>/app.nexus.desktop/` | вне vault | согласие на ленту новостей (десктоп); нет → выключено |
| **`boards/<id>.json`** | `<vault>/.nexus/boards/` | в vault | конфиг канбан-доски; нет → дефолтная доска |

**`<OS-config-dir>` по платформам:**

| ОС | Путь |
|---|---|
| macOS | `~/Library/Application Support/app.nexus.desktop/` |
| Linux | `~/.config/app.nexus.desktop/` |
| Windows | `%APPDATA%\app.nexus.desktop\` |

Безголовый `nexus-agentd` читает ту же OS-config-dir через `dirs::config_dir()` либо override `NEXUS_CONFIG_DIR`. Чтобы kill-switch'и (`egress.json`/`agent.json`) работали и в десктопе, и в agentd, путь должен совпадать.

**Принцип fail-safe (по построению):** отсутствует → разумные дефолты (фичи OFF); битый JSON → warning + дефолты; неизвестные поля → игнорируются (forward-compatible, `#[serde(default)]`); невалидные значения → security-гейты блокируют опасное (fail-closed).

**Почему `local.json` в .gitignore (ADR-002):** чтобы синхронизированный через git vault НЕ расширял молча сетевую/sandbox-поверхность на другом устройстве — эндпоинты и опасные флаги задаются локально на каждом устройстве.

---

## `.nexus/local.json` — конфиг агента/ИИ

Корень — объект с единственным ключом `ai` (всё дерево настроек ИИ/агента). Прочие ключи игнорируются (forward-compatible).

```json
{ "ai": { /* ... */ } }
```

### LLM-эндпоинты

`ai.chat` (объект, опционален; без него чат недоступен). Главная reasoning-модель (llama.cpp/vLLM/OpenAI-совместимый).

| Путь | Тип | Дефолт | Назначение |
|---|---|---|---|
| `ai.chat.url` | string | — (обязателен, если секция есть) | HTTP(S) base-URL чат-провайдера. Хост авто-добавляется в egress-allowlist; проверяется SSRF-гардом. |
| `ai.chat.model` | string | `null` | Имя модели для роутинга (напр. `qwen3.6-27b-awq-mtp`). |
| `ai.chat.context_window` | usize | `null` → 32768 | Размер контекста модели (для бюджета токенов). |
| `ai.chat.reserve_output_tokens` | usize | `null` → 1024 | Сколько токенов резервируется под вывод (вычитается из окна). |
| `ai.chat.temperature` | f32 | `null` → 0.3 | Температура сэмплинга (консервативно для reasoning). |
| `ai.chat.first_token_timeout_secs` | u64 | `null` → 300 | Таймаут до первого токена (терпит cold-start/компиляцию кернелов на V100). |
| `ai.chat.idle_timeout_secs` | u64 | `null` → 90 | Таймаут простоя стрима после первого байта. |
| `ai.chat.connect_timeout_secs` | u64 | `null` → 30 | TCP-connect таймаут. |
| `ai.chat.retry_attempts` | u32 | `null` → 3 | Число попыток инициации запроса (включая первую). |

`ai.embedding` (объект, опционален; нужен для RAG/поиска по смыслу). Отдельный хост (ADR-005).

| Путь | Тип | Дефолт | Назначение |
|---|---|---|---|
| `ai.embedding.url` | string | — (обязателен, если секция есть) | Base-URL эмбеддера. |
| `ai.embedding.model` | string | `null` | Имя модели эмбеддинга. |
| `ai.embedding.dim` | usize | `null` → детект из первого ответа | Размерность эмбеддинга. |
| `ai.embedding.timeout_secs` | u64 | `null` → 60 | Таймаут батч-запроса эмбеддинга. |

`ai.fast` (объект, опционален). Маленькая быстрая модель (inline/judge/новости); без неё эти задачи падают на главный `ai.chat`. Поля те же, что у `ai.chat`.

`ai.tokenizer_path` (string, `null`) — путь к кастомному `tokenizer.json` для точного бюджета контекста (P0-c). По умолчанию используется токенайзер задеплоенной модели.

### Актуатор (запись в vault) — SAFE-by-default

| Путь | Тип | Дефолт | Назначение / гейтинг |
|---|---|---|---|
| `ai.agent_actuator_enabled` | bool | **`false`** | **Мастер-свитч реальных действий в vault.** OFF → инструменты-заглушки (никакой записи). ON → реальные note.create/edit/set_frontmatter ТОЛЬКО через actuator-гейт (classify→autonomy→approval→snapshot→undo). |
| `ai.agent_autonomy` | string | `null` → `"confirm"` | Режим автономии безголового агента: `"confirm"` (человек-в-петле, безопасно) или `"auto"` (авто-применяет Auto-тир: low-risk + undo). Действует лишь при `agent_actuator_enabled=true`. |
| `ai.agent_overwrite_threshold` | usize | `null` → 65536 (64 КиБ) | Порог «большой перезаписи» → форс Confirm-тира. |
| `ai.agent_blast_radius_cap` | u32 | `null` → 16 | Кап накопленных Auto-действий за прогон (anti-fatigue): после капа даже Auto-тир форсит proposal. |

### Скиллы — SAFE-by-default

| Путь | Тип | Дефолт | Назначение / гейтинг |
|---|---|---|---|
| `ai.agent_skills_dir` | string | `null` → без скиллов | Путь к каталогу скиллов (`<dir>/<skill>/SKILL.md`). Без него агент работает без скиллов (без регрессии). |
| `ai.skills.learning_enabled` | bool | **`false`** | **Самообучение (owner-gated).** OFF → `skill.save` HardBlocked, curator не работает. ON → агент МОЖЕТ предлагать запись скиллов (НИКОГДА не Auto). |

### Веб-инструменты — SAFE-by-default

| Путь | Тип | Дефолт | Назначение / гейтинг |
|---|---|---|---|
| `ai.web.enabled` | bool | **`false`** | Согласие на веб (EGR-AGENT-2). OFF → web.search/web.fetch не регистрируются. |
| `ai.web.url` | string | `""` | Base-URL инстанса SearXNG. Пустой → веб инертен даже при `enabled=true`. Эгресс через `GuardedClient` (SSRF-guard, allowlist). |
| `ai.web.allow_public_fetch` | bool | **`false`** | **Публичный эгресс (owner-gated).** OFF → только allowlist. ON → web.fetch на любой публичный URL (для deep-research); всё равно под guard (deny_private/SSRF/audit). |

### Песочница и host-exec — SAFE-by-default + Linux-only

| Путь | Тип | Дефолт | Назначение / гейтинг |
|---|---|---|---|
| `ai.sandbox_enabled` | bool | **`false`** | **Мастер-свитч OS-песочницы** (rootless-Podman `--network=none`). OFF → агент in-process (текущее поведение). Не-Linux → флаг инертен. |
| `ai.shell_enable` | bool | **`false`** | **Гейт host-exec (owner-gated).** OFF → exec-таргеты (ShellRun/ProcessSpawn/GitOp) HardBlocked. ON → classify=Confirm (НИКОГДА не Auto), исполнение в песочнице после host-approval. Требует `sandbox_enabled=true` И Linux. См. [THREAT_MODEL.md](THREAT_MODEL.md) T7. |
| `ai.git_worktree` | string | `null` → undo Deferred | Writable git-repo для отката exec-GitOp (owner-gated). Без него undo exec-GitOp откладывается; путь монтируется отдельно rw (vault всегда :ro). |

> Связность форсится на уровне сохранения настроек: `shell_enable=true` не сохраняется без `sandbox_enabled=true` (`commands/settings.rs`).

### Делегирование / субагенты — SAFE-by-default + owner-gated

| Путь | Тип | Дефолт | Назначение |
|---|---|---|---|
| `ai.delegation.enabled` | bool | **`false`** | Гейт субагентов. OFF → `delegate.run` не регистрируется. |
| `ai.delegation.max_depth` | usize | `1` | Глубина дерева делегирования (1 = плоско: родитель→ребёнок; внук режется). |
| `ai.delegation.max_fanout` | usize | `3` | Макс. детей за один `delegate.run` (превышение — recoverable tool-error). |
| `ai.delegation.max_total_spawns` | usize | `8` | Кап спавнов за прогон (общий счётчик по дереву, anti-runaway). |

### Deep-research — SAFE-by-default + owner-gated

| Путь | Тип | Дефолт | Назначение |
|---|---|---|---|
| `ai.research.enabled` | bool | **`false`** | Гейт deep-research. OFF → `research.run` не регистрируется. ON требует ещё `ai.delegation.enabled=true` И веб. |
| `ai.research.max_rounds` | u8 | `3` | Итерации decompose→fan-out→synthesize (hard-cap ~8 в оркестраторе; 0→1). |
| `ai.research.max_urls_per_round` | usize | `3` | Макс. URL за раунд (anti-token-flood / anti-egress на одной GPU). |
| `ai.research.max_content_chars` | usize | `15000` | Кап извлечённого контента на страницу (anti-OOM). |
| `ai.research.extraction_concurrency` | usize | `3` | Семафор параллельного извлечения (backpressure на одной GPU). |
| `ai.research.wall_clock_secs` | u64 | `0` → дефолт раннера | Таймаут durable-джобы (оркестратор клампит 60..86400). |

Эндпоинты `ai.chat.url`/`ai.embedding.url`/`ai.fast.url` автоматически попадают в egress-allowlist скоупа `ai` (невалидные URL пропускаются, fail-closed); пустой конфиг → пустой allowlist (никаких публичных хостов по умолчанию).

---

## Файлы-kill-switch (OS-config-dir, вне vault)

### `egress.json` — рубильник сети

| Поле | Тип | Дефолт | Назначение |
|---|---|---|---|
| `offline` | bool | `false` | Kill-switch: `true` → публичные хосты заблокированы (LAN/loopback живы). |
| `chat` | bool | `true` | Тоггл фичи чата (local-first: по умолчанию ON). |
| `embed` | bool | `true` | Тоггл эмбеддинга. |
| `probe` | bool | `true` | Тоггл сетевых проб. |

### `agent.json` — пауза агента

| Поле | Тип | Дефолт | Назначение |
|---|---|---|---|
| `paused` | bool | `false` | KILL-SWITCH: `true` → агент на паузе (задачи в очереди, исполнения нет). Битый/нет → unpause (агент работает из коробки). |

### `websearch.json` (десктоп) и `news.json` (десктоп)

`websearch.json`: `enabled` (bool, `false`), `url` (string, `""`). `news.json`: `enabled` (bool, `false`), `sources` (map id→bool, `{}`), `keywords` ([string], `null`→пресет), `extra_hosts` ([string], `[]`, каждый хост одобряется явно в UI). Безголовый agentd берёт веб из `ai.web` в `local.json`.

---

## Канбан-доски: `boards/<id>.json`

`id` (из имени файла — источник истины), `title` (`""`), `statusKey` (`"status"`), `columns` (дефолт `todo`/`doing`/`done`), `scope` (фильтр папка/проект/тег), `order` (ручной порядок карточек), `sort` (`"manual"`), `cardFields` (`["due","priority","tags"]`). Нет файла → дефолтная доска `personal`; битый JSON → дефолт + флаг `corrupt` (UI показывает тост, не обрезает).

---

## Поведение загрузки и валидация

- **Парсинг `local.json`** (`LocalConfig::parse`): частичные/неизвестные поля игнорируются; нет секции `ai` → `AiConfig::default()` (всё None/false); невалидный JSON → ошибка, загрузчик деградирует (agentd логирует «нет .nexus/local.json» и продолжает).
- **Hot-reload:** чат-провайдер перечитывается на следующий запрос; эмбеддер НЕ hot-reload (индекс зависит от стабильной модели → нужен рестарт); флаги sandbox/shell/autonomy применяются на следующем прогоне агента.
- **Валидация:** `*.url` → SSRF/DNS-rebind guard на эгрессе (fail-closed); `agent_autonomy` неизвестное значение → не сохраняется, дефолт `confirm`; `shell_enable` некогерентный (без sandbox) не сохраняется; `research.max_rounds=0` → клам к 1; не-Linux → sandbox/shell структурно инертны.

## Заметки по безопасности

- Все агент/актуатор/sandbox/shell/research/delegation/web фичи **OFF по умолчанию** — владелец включает явно.
- Опасные включения (`shell_enable`, `web.allow_public_fetch`, `skills.learning_enabled`, `delegation.enabled`, `research.enabled`, `git_worktree`) — **owner-gated**; см. [THREAT_MODEL.md](THREAT_MODEL.md) (T7/T9/T10/T11) и фаза-гейты.
- `local.json` gitignored (ADR-002): синхронизация vault не расширяет сетевую/exec-поверхность на другом устройстве.

## Пример минимального `local.json`

```json
{
  "ai": {
    "chat": { "url": "http://192.168.0.28:8080", "model": "qwen3.6-27b-awq-mtp" },
    "embedding": { "url": "http://192.168.0.28:8083", "model": "bge-m3", "dim": 1024 }
  }
}
```

## Пример с включённым агентом (vault-only, человек-в-петле)

```json
{
  "ai": {
    "chat": { "url": "http://192.168.0.28:8080", "model": "qwen3.6-27b-awq-mtp" },
    "embedding": { "url": "http://192.168.0.28:8083", "model": "bge-m3", "dim": 1024 },
    "fast": { "url": "http://192.168.0.28:8084", "model": "gemma-e4b" },
    "agent_actuator_enabled": true,
    "agent_autonomy": "confirm",
    "agent_skills_dir": "_skills"
  }
}
```

> Опасные способности (`web.allow_public_fetch`, `shell_enable`, `sandbox_enabled`, `delegation.enabled`, `research.enabled`, `skills.learning_enabled`) здесь НЕ включены — добавляйте их осознанно, по одному, прочитав соответствующий раздел THREAT_MODEL.
