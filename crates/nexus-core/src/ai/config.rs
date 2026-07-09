//! Локальный конфиг vault (`.nexus/local.json`, в .gitignore — ADR-002): эндпоинты/модели
//! chat и embedding. Ключи здесь НЕ в git; `*.url` валидируются анти-SSRF позже (§11).

use std::time::Duration;

use serde::Deserialize;

use super::{AiError, AiResult};

#[derive(Debug, Clone, Default, Deserialize)]
pub struct LocalConfig {
    #[serde(default)]
    pub ai: AiConfig,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct AiConfig {
    /// Chat-провайдер (Gemma и т.п.) — отдельный хост (ADR-005).
    pub chat: Option<ChatConfig>,
    /// Embedding-провайдер (мультиязычный) — отдельный хост (ADR-005).
    pub embedding: Option<EmbeddingConfig>,
    /// «Быстрая» утилитарная модель (мелкая, напр. Qwen3-4B на отдельном порту) для примитивов
    /// (inline/судья): низкая латентность + разгрузка основной модели. Опционально — нет секции →
    /// fallback на основной chat без reasoning. Non-reasoning-модель → шлём обычный запрос.
    pub fast: Option<ChatConfig>,
    /// Путь к `tokenizer.json` для оценки бюджета контекста (P0-c). `None` → встроенный токенайзер
    /// задеплоенной модели (Qwen3.6-27B). Смена модели = положить новый файл + прописать этот путь,
    /// без пересборки (см. `ai::QwenTokenizer`). Относительный путь резолвится вызывающим.
    #[serde(default)]
    pub tokenizer_path: Option<String>,

    /// **GO-LIVE АКТУАТОРА (AGENT-3e), SAFE BY DEFAULT.** Когда `false` (ДЕФОЛТ) — прогон агента
    /// работает БЕЗ инструментов записи (реестр ПУСТ, если не подключены read-only skills/web — B7);
    /// реальный vault НИКОГДА не затрагивается из
    /// коробки. Когда `true` — [`crate::agent::AgentRunHandler`] регистрирует файловые инструменты-
    /// актуаторы (note.create/edit/set_frontmatter), маршрутизируемые ИСКЛЮЧИТЕЛЬНО через гейт
    /// автономии (`actuator::dispatch_action`). Даже включённый, headless-agentd под `PolicyDefault`
    /// авто-применяет лишь Auto-тир на `autonomy=auto`-прогоне; Confirm-тир всегда предлагается и
    /// auto-DENY-отклоняется (нет UI/контрол-плейна). Владелец opt-in'ит осознанно.
    #[serde(default)]
    pub agent_actuator_enabled: bool,

    /// **Автономия серверного (headless) прогона агента через коннектор** (`"confirm"` | `"auto"`).
    /// `None`/невалидно → `"confirm"` (SAFE-default, человек-в-петле для интерактивного десктопа).
    /// `"auto"` (owner-gated 2026-06-22, headless-сервер): агент САМ авто-применяет Auto-тир актуатора
    /// (low-risk, blast-cap+undo+audit); Confirm-тир (риск/крупная перезапись) НЕ авто-применяется — он
    /// ПРЕДЛАГАЕТСЯ по проводу (Proposal) и пишется лишь по явному `agent/approve` (fail-closed reject_all
    /// при дисконнекте клиента). Эффект только при `agent_actuator_enabled=true`. → `ConnectDeps::autonomy`.
    #[serde(default)]
    pub agent_autonomy: Option<String>,

    /// Порог «крупной перезаписи» (байт) для гейта актуатора → Confirm-тир (`DispatchPolicy
    /// .overwrite_threshold`). `None` → дефолт [`crate::actuator::OVERWRITE_THRESHOLD`] (64 KiB).
    /// Только при `agent_actuator_enabled=true` имеет эффект.
    #[serde(default)]
    pub agent_overwrite_threshold: Option<usize>,

    /// Кэп кумулятивных авто-применений Auto-тира В ПРОГОНЕ (анти-усталость): за ним даже Auto-тир
    /// форсирует предложение. `None` → дефолт [`AiConfig::DEFAULT_BLAST_RADIUS_CAP`]. Только при
    /// `agent_actuator_enabled=true` имеет эффект.
    #[serde(default)]
    pub agent_blast_radius_cap: Option<u32>,

    /// **Конфигурируемый `wall_clock` прогона агента (хвост BF-1).** Стенной бюджет ВСЕГО прогона (сек):
    /// ручка владельца на remaining-бюджет реального времени работы (BF-1 уже исключил время ожидания
    /// человека у гейта). `None` (ДЕФОЛТ) → [`crate::agent::LoopBounds::default`] (300 с). Санитарный кламп
    /// [`AiConfig::MIN_AGENT_WALL_CLOCK_SECS`]..[`AiConfig::MAX_AGENT_WALL_CLOCK_SECS`] применяет геттер
    /// [`AiConfig::agent_wall_clock`]. Тип-толерантный десериализатор (см. [`de_tolerant_opt_u64`]):
    /// мусорный тип (строка/булево/массив) → `None`, НЕ ошибка парса — не роняет `ai.chat`/`ai.embedding`.
    #[serde(default, deserialize_with = "de_tolerant_opt_u64")]
    pub agent_wall_clock_secs: Option<u64>,

    /// **Конфигурируемый `max_steps` прогона агента (хвост BF-1).** Потолок ходов модели (анти-зацикливание).
    /// `None` (ДЕФОЛТ) → [`crate::agent::LoopBounds::default`] (8). Идёт в паре с `agent_wall_clock_secs`:
    /// длинный wall_clock без большего числа шагов — половина ручки. Кламп [`AiConfig::MIN_AGENT_MAX_STEPS`]
    /// ..[`AiConfig::MAX_AGENT_MAX_STEPS`] в геттере [`AiConfig::agent_max_steps`]. Тип-толерантно (как выше).
    #[serde(default, deserialize_with = "de_tolerant_opt_u64")]
    pub agent_max_steps: Option<u64>,

    /// **SKILL-2: каталог скиллов (SKILL.md) для прогона агента.** Путь к каталогу со скиллами
    /// открытого стандарта SKILL.md (`<dir>/<skill>/SKILL.md`). `None` (ДЕФОЛТ) → агент работает БЕЗ
    /// скиллов (нет меню в контексте, нет `activate_skill`/`read_skill_resource` — поведение без
    /// регрессии). Когда задан → [`crate::agent::AgentRunHandler`] инжектит фенсенное МЕНЮ скиллов
    /// (tier 1) и регистрирует READ-ONLY инструменты раскрытия (tier 2/3). Скиллы лишь читаются;
    /// активация даёт ТОЛЬКО текст-инструкции (capability-гейт — SKILL-3). Относительный путь
    /// резолвится вызывающим относительно vault (рекомендация: `<vault>/.nexus/skills`).
    #[serde(default)]
    pub agent_skills_dir: Option<String>,

    /// **EGR-AGENT: веб-инструменты агента (`web.search`/`web.fetch`).** `None`/`enabled=false` (ДЕФОЛТ) →
    /// агент без веб-доступа. Задан+enabled → composition root включает `EgressFeature::Web` + allowlist
    /// хоста SearXNG и регистрирует read-only веб-инструменты. Эгресс — через `GuardedClient` (web-класс:
    /// SSRF-гард, allowlist, аудит). Только для прогона агента; chat-путь не затрагивает.
    #[serde(default)]
    pub web: Option<WebConfig>,

    /// **SANDBOX-1 (Фаза-2 каркас), SAFE BY DEFAULT.** Мастер-свитч OS-песочницы прогона агента
    /// (`docs/specs/agent-sandbox.md`). `false` (ДЕФОЛТ) → агент бежит in-process через
    /// [`crate::agent::AgentRunHandler`], поведение байт-в-байт сегодняшнее. `true` → (по мере поставки
    /// срезов SANDBOX-2..5) прогон исполняется в эфемерном rootless-Podman `--network=none` контейнере,
    /// эгресс — только через host-side GuardedProxy поверх существующего `GuardedClient`. Фича
    /// Linux-host-only; на не-Linux флаг структурно инертен. На этом срезе (SANDBOX-1) флаг ещё НЕ
    /// меняет рантайм — только декларирован + используется чистым рендером плана `sandbox::sandbox_run_plan`.
    #[serde(default)]
    pub sandbox_enabled: bool,

    /// **SANDBOX-6a (Фаза-3 host-actuator), SAFE BY DEFAULT + OWNER-GATED.** Гейт исполнения host
    /// exec-таргетов (`ShellRun`/`ProcessSpawn`/`GitOp` — приходят в SANDBOX-6b) ВНУТРИ песочницы.
    /// `false` (ДЕФОЛТ) → exec-таргеты `classify` → `HardBlocked(ShellDisabled)`, `host/exec` инертен;
    /// `true` → exec-таргеты `classify` → `Confirm` (НИКОГДА `Auto`), исполняются in-sandbox после
    /// host-апрува (`docs/specs/agent-sandbox.md §5/§T7`). Требует `sandbox_enabled` И Linux: на не-Linux
    /// / при выключенной песочнице exec-таргеты → `HardBlocked(SandboxUnavailable)` (block by-construction).
    /// На этом срезе (6a) флаг ещё НЕ рождает exec-таргеты (их вводит 6b) — только декларирован + питает
    /// env-scrub-allowlist рендера и будущий classify.
    #[serde(default)]
    pub shell_enable: bool,

    /// **SANDBOX-6c-3d, OWNER-GATED, default None.** Опц. ПЕРСИСТЕНТНЫЙ writable git-worktree для РЕАЛЬНОГО
    /// отката exec-GitOp (`git reset --hard <pre-op-ref>`, см. [`crate::actuator::UndoExecDriver`]). `None`
    /// (ДЕФОЛТ) → exec-GitOp откат остаётся `Deferred` (vault `:ro`, scratch-tmpfs эфемерен — кросс-прогонный
    /// reset невозможен). `Some(path)` → этот каталог монтируется ОТДЕЛЬНЫМ rw-маунтом (НИКОГДА не vault!) в
    /// undo-контейнер, где и выполняется reset. **Новая security-поверхность** (writable repo в песочнице) —
    /// включает ТОЛЬКО владелец явной конфигурацией; vault остаётся `:ro` всегда.
    #[serde(default)]
    pub git_worktree: Option<String>,

    /// **SELF-LEARNING (SL-7), SAFE BY DEFAULT + OWNER-GATED.** Настройки самообучения навыкам. NON-Option
    /// (всегда есть, дефолт-OFF — нет None-неоднозначности): отсутствие `ai.skills` в конфиге = всё false.
    #[serde(default)]
    pub skills: SkillsConfig,

    /// **SUBAGENTS (SUB-0), SAFE BY DEFAULT + OWNER-GATED.** Делегирование/субагенты. NON-Option (всегда
    /// есть, дефолт-OFF): отсутствие `ai.delegation` в конфиге = `enabled=false` + консервативные капы.
    #[serde(default)]
    pub delegation: DelegationConfig,

    /// **DEEP-RESEARCH (RES-1), SAFE BY DEFAULT + OWNER-GATED.** Многораундовый веб-ресёрч (план→fan-out
    /// read-only воркеров→синтез→отчёт-в-vault через гейт). NON-Option (всегда есть, дефолт-OFF): отсутствие
    /// `ai.research` = `enabled=false` + консервативные капы. Инструмент `research.run` регистрируется ТОЛЬКО
    /// при `enabled` И `ai.delegation.enabled` И включённом web (RES-4) — структурно инертен иначе.
    #[serde(default)]
    pub research: ResearchConfig,

    /// **CONN-1 (ACP/расцепление, фундамент), SAFE BY DEFAULT.** Как app/agentd получает агент-бэкенд.
    /// Отсутствие `ai.connection` → embedded (serde Default) = байт-в-байт сегодняшнее поведение (агент
    /// in-process). Connected/ACP-транспорты приходят в CONN-2+; на этом срезе подключён ТОЛЬКО Embedded.
    #[serde(default)]
    pub connection: ConnectionConfig,
}

/// Выбор агент-бэкенда (CONN-1). Дефолт — embedded (in-process), без регрессии. `mode` — `Option<String>`
/// (НЕ enum) НАМЕРЕННО: мусорное/неизвестное значение НИКОГДА не уронит `LocalConfig::parse` (не потеряем
/// chat/embedding-конфиг) — нормализуется в [`ConnectionMode::Embedded`] (как `agent_autonomy`/normalize).
/// Толерантный к ТИПУ десериализатор `Option<String>`: любое НЕ-строковое значение (число/булево/
/// массив/объект) → `None` вместо ОШИБКИ парса. Keystone-защита: мусорный `ai.connection.*` (напр.
/// `"mode": 42` при ручной правке) НЕ должен ронять весь `LocalConfig::parse` и терять `ai.chat`/
/// `ai.embedding` (тот же класс data-loss, на котором проект горел — см. `WebConfig.url`). Голый
/// `Option<String>` + `#[serde(default)]` спасает только от ОТСУТСТВИЯ поля, не от неверного типа.
fn de_tolerant_opt_string<'de, D>(d: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let v = Option::<serde_json::Value>::deserialize(d)?;
    Ok(v.and_then(|val| val.as_str().map(str::to_string)))
}

/// Тип-толерантный десериализатор `Option<Vec<String>>` (ACP-1, для `ai.connection.acp_command`): принимает
/// ТОЛЬКО массив строк; любое иное (число/объект/строка/массив-с-не-строками) → `None`, НЕ ошибка парса
/// (та же data-loss-защита, что у [`de_tolerant_opt_string`] — мусорный `acp_command` не роняет `ai.chat`).
fn de_tolerant_string_vec<'de, D>(d: D) -> Result<Option<Vec<String>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let v = Option::<serde_json::Value>::deserialize(d)?;
    Ok(v.and_then(|val| match val {
        serde_json::Value::Array(items) => {
            let strs: Vec<String> = items
                .iter()
                .filter_map(|i| i.as_str().map(str::to_string))
                .collect();
            // Все элементы должны быть строками И массив непустой, иначе команда бессмысленна → None.
            if !strs.is_empty() && strs.len() == items.len() {
                Some(strs)
            } else {
                None
            }
        }
        _ => None,
    }))
}

/// Тип-толерантный десериализатор `Option<u64>` (BF-1, для `ai.agent_wall_clock_secs`/`ai.agent_max_steps`):
/// принимает ТОЛЬКО JSON-число, влезающее в `u64` (неотрицательное целое); любое иное — строка (в т.ч.
/// `"42"`), булево, массив, объект, отрицательное или дробное — → `None`, НЕ ошибка парса. Та же
/// data-loss-защита, что у [`de_tolerant_opt_string`]: мусорный `agent_wall_clock_secs` (ручная правка
/// `local.json`) НЕ должен ронять весь [`LocalConfig::parse`] и терять `ai.chat`/`ai.embedding`.
/// Санитарный кламп значения — НЕ здесь, а в геттерах [`AiConfig::agent_wall_clock`]/[`AiConfig::agent_max_steps`].
fn de_tolerant_opt_u64<'de, D>(d: D) -> Result<Option<u64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let v = Option::<serde_json::Value>::deserialize(d)?;
    Ok(v.and_then(|val| val.as_u64()))
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ConnectionConfig {
    /// `"embedded"` (ДЕФОЛТ) | `"local"` (AF_UNIX-сокет, CONN-2) | `"remote"` (url+токен, CONN-3).
    /// `None`/неизвестное → embedded. Тип-толерантный десериализатор: мусор → None, не роняет конфиг.
    #[serde(default, deserialize_with = "de_tolerant_opt_string")]
    pub mode: Option<String>,
    /// AF_UNIX-путь для `mode="local"` (CONN-2). Игнорируется для embedded.
    #[serde(default, deserialize_with = "de_tolerant_opt_string")]
    pub socket: Option<String>,
    /// URL для `mode="remote"` (CONN-3). Игнорируется для embedded.
    #[serde(default, deserialize_with = "de_tolerant_opt_string")]
    pub url: Option<String>,
    /// Ссылка на секрет/токен auth (keyring ref / env, CONN-3). Игнорируется для embedded.
    #[serde(default, deserialize_with = "de_tolerant_opt_string")]
    pub auth_ref: Option<String>,
    /// ACP-1: программа+аргументы для спавна внешнего ACP-агента, напр. `["hermes","acp"]`. Только для
    /// `mode="acp"`. Тип-толерантно: не-массив/не-строки/пусто → None (не роняет конфиг).
    #[serde(default, deserialize_with = "de_tolerant_string_vec")]
    pub acp_command: Option<Vec<String>>,
    /// ACP-1: рабочий каталог (`cwd`) для ACP-сессии (`session/new`). Дефолт — корень vault. Только `mode="acp"`.
    #[serde(default, deserialize_with = "de_tolerant_opt_string")]
    pub acp_cwd: Option<String>,
    /// ACP-транспорт: `"local"` (ДЕФОЛТ, спавн `acp_command`) | `"ssh"` (собрать ssh-команду из полей ниже).
    /// `None`/неизвестное → как `"local"` (см. [`ConnectionConfig::acp_spawn_argv`]). Тип-толерантно.
    #[serde(default, deserialize_with = "de_tolerant_opt_string")]
    pub acp_transport: Option<String>,
    /// SSH: `"user@host"` (для `acp_transport="ssh"`).
    #[serde(default, deserialize_with = "de_tolerant_opt_string")]
    pub acp_ssh_host: Option<String>,
    /// SSH: путь к приватному ключу (опц.; пусто → ключ по умолчанию ssh).
    #[serde(default, deserialize_with = "de_tolerant_opt_string")]
    pub acp_ssh_key: Option<String>,
    /// SSH: команда запуска ACP-сервера НА ХОСТЕ, напр. `"docker exec -i hermes hermes acp"` (split по пробелам).
    #[serde(default, deserialize_with = "de_tolerant_opt_string")]
    pub acp_remote_command: Option<String>,
}

/// Нормализованный режим коннекта. `Default` = `Embedded` → отсутствие/неизвестное значение безопасно.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ConnectionMode {
    #[default]
    Embedded,
    Local,
    Remote,
    /// ACP-1: внешний ACP-агент (Hermes и пр.), спавнится подпроцессом по `acp_command`.
    Acp,
}

impl ConnectionConfig {
    /// Нормализованный режим: `None`/неизвестное → [`ConnectionMode::Embedded`] (SAFE-default — мусорный
    /// `mode` не активирует внешний транспорт и не роняет конфиг).
    pub fn mode(&self) -> ConnectionMode {
        match self.mode.as_deref() {
            Some("local") => ConnectionMode::Local,
            Some("remote") => ConnectionMode::Remote,
            Some("acp") => ConnectionMode::Acp,
            _ => ConnectionMode::Embedded,
        }
    }

    /// ACP-REMOTE-SSH: итоговый argv для спавна ACP-агента. При `acp_transport="ssh"` собирает
    /// `ssh [-i key] user@host <remote_command…>` (remote split по пробелам, БЕЗ shell-quoting — как
    /// `acp_command`); иначе возвращает локальный `acp_command`. `None` → не сконфигурировано
    /// (для ssh не задан host/команда; для local пуст `acp_command`) — вызывающий выдаёт внятную ошибку.
    ///
    /// SSH-опции `StrictHostKeyChecking=no`+`BatchMode=yes`: спавн НЕинтерактивный (stdin занят ACP
    /// JSON-RPC) — без BatchMode ssh завис бы на парольном/known-hosts промпте; host-key-проверка
    /// в headless-сценарии (Docker на LAN) только повесила бы первый коннект.
    pub fn acp_spawn_argv(&self) -> Option<Vec<String>> {
        if self.acp_transport.as_deref() == Some("ssh") {
            let host = self
                .acp_ssh_host
                .as_deref()
                .filter(|s| !s.trim().is_empty())?;
            let remote = self
                .acp_remote_command
                .as_deref()
                .filter(|s| !s.trim().is_empty())?;
            let mut argv = vec![
                "ssh".into(),
                "-o".into(),
                "StrictHostKeyChecking=no".into(),
                "-o".into(),
                "BatchMode=yes".into(),
            ];
            if let Some(key) = self.acp_ssh_key.as_deref().filter(|s| !s.trim().is_empty()) {
                argv.push("-i".into());
                argv.push(key.into());
            }
            argv.push(host.into());
            argv.extend(remote.split_whitespace().map(str::to_string));
            Some(argv)
        } else {
            self.acp_command.clone().filter(|v| !v.is_empty())
        }
    }
}

/// Конфиг самообучения навыкам (SELF-LEARNING). Дефолт-OFF: пустой `ai.skills` → `learning_enabled=false`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct SkillsConfig {
    /// **SL-7, OWNER-GATED, ДЕФОЛТ false.** Гейт ДЕЙСТВИЙ самообучения: `skill_save`-инструмент
    /// (авторство SKILL.md агентом) + будущая scheduler-джоба curator'а (lifecycle навыков). `false` →
    /// `SkillSave` `classify` → `HardBlocked(LearningDisabled)`, инструмент НЕ регистрируется, curator
    /// спит. `true` → агент может ПРЕДЛОЖИТЬ сохранить навык (НИКОГДА `Auto` — всегда апрув). НЕ гейтит
    /// телеметрию использования (`agent_skill_usage` пишется всегда — чистая наблюдаемость, SL-2).
    #[serde(default)]
    pub learning_enabled: bool,
}

/// Конфиг делегирования/субагентов (SUBAGENTS, SUB-0). Дефолт-OFF + консервативные капы (hermes-tuned):
/// отсутствие `ai.delegation` = `enabled=false`, `max_depth=1`, `max_fanout=3`, `max_total_spawns=8`.
/// `#[serde(default)]` на контейнере → недостающие поля берутся из [`Default`] (частичный конфиг ОК, и
/// каждое поле падает в свой безопасный дефолт, а не в 0). Капы НЕнулевые → ручной `impl Default`.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct DelegationConfig {
    /// **OWNER-GATED, ДЕФОЛТ false.** Гейт делегирования: инструмент `delegate.run` регистрируется ТОЛЬКО
    /// при `true`. `false` → субагентов нет, поведение без регрессии (инструмент структурно отсутствует).
    pub enabled: bool,
    /// Макс. ГЛУБИНА дерева делегирования. hermes `MAX_DEPTH=1` (плоско parent→child; внук отвергается —
    /// `delegate.run` структурно вырезан из реестра ребёнка + depth-гейт бюджета, два чекпоинта).
    pub max_depth: usize,
    /// Макс. детей за ОДИН вызов `delegate.run` (hermes `_DEFAULT_MAX_CONCURRENT_CHILDREN=3`). Батч сверх
    /// — recoverable-ошибка инструмента (не спавним).
    pub max_fanout: usize,
    /// Макс. СУММАРНО спавнов за прогон (общий счётчик дерева, анти-runaway).
    pub max_total_spawns: usize,
}

impl Default for DelegationConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            max_depth: 1,
            max_fanout: 3,
            max_total_spawns: 8,
        }
    }
}

/// Конфиг deep-research (DEEP-RESEARCH, RES-1). Дефолт-OFF + консервативные капы (odysseus-tuned, под один
/// локальный GPU): отсутствие `ai.research` = `enabled=false`, `max_rounds=3`, `max_urls_per_round=3`,
/// `max_content_chars=15000`, `extraction_concurrency=3`, `wall_clock_secs=0` (0 → дефолт раннера). Капы
/// НЕнулевые → ручной `impl Default`. `#[serde(default)]` на контейнере → частичный конфиг падает в дефолты
/// полей. `max_rounds` — мягкий потолок ~8 форсит RES-3-оркестратор (здесь только сериализуемое значение).
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ResearchConfig {
    /// **OWNER-GATED, ДЕФОЛТ false.** Гейт deep-research: инструмент `research.run` регистрируется ТОЛЬКО
    /// при `true` (И `ai.delegation.enabled` И включённом web — RES-4). `false` → ресёрча нет (инструмент
    /// структурно отсутствует, без регрессии).
    pub enabled: bool,
    /// Макс. РАУНДОВ итеративного ресёрча (decompose→fan-out→synthesize→stop). Дефолт 3; жёсткий потолок
    /// (~8) применяет оркестратор (RES-3). 0 НЕ допускается логикой — clamp в RES-3 к минимум 1.
    pub max_rounds: u8,
    /// Макс. URL, забираемых за ОДИН раунд (анти-токен-флуд + анти-эгресс на одном GPU). odysseus-дефолт 3.
    pub max_urls_per_round: usize,
    /// Кэп символов извлекаемого контента страницы перед подачей в экстракт-промпт (анти-OOM/токен-флуд).
    pub max_content_chars: usize,
    /// Параллелизм извлечения (Semaphore-backpressure воркеров) — критично на одном локальном GPU. Дефолт 3.
    pub extraction_concurrency: usize,
    /// Стен-таймаут durable-джобы ресёрча (RES-5), секунды. 0 → дефолт раннера; clamp 60..86400 применяет
    /// джоба (здесь только сырое значение конфига).
    pub wall_clock_secs: u64,
}

impl Default for ResearchConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            max_rounds: 3,
            max_urls_per_round: 3,
            max_content_chars: 15000,
            extraction_concurrency: 3,
            wall_clock_secs: 0,
        }
    }
}

/// Конфиг веб-инструментов агента (EGR-AGENT-2). `url` — база SearXNG (consent-эндпоинт мета-поиска).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct WebConfig {
    /// База SearXNG (например `http://host:8888`). Пусто → web.search не поднимается. `#[serde(default)]`:
    /// частичный `ai.web` (напр. только `allow_public_fetch` из тоггла настроек) парсится с пустым URL и
    /// остаётся ИНЕРТНЫМ (агентд поднимает веб лишь при `enabled && !url.is_empty()`) — без этого
    /// присутствие `ai.web` без `url` валило бы парс всего `local.json` (потеря chat/embedding-конфига).
    #[serde(default)]
    pub url: String,
    /// Consent-флаг (ДЕФОЛТ false): без него веб-инструменты не регистрируются.
    #[serde(default)]
    pub enabled: bool,
    /// **WEB-FETCH-PUBLIC (owner-gated 2026-06-22):** снимает allowlist-требование для egress-фичи `Web`
    /// → `web.fetch` к ЛЮБОМУ публичному URL (для deep-research; `web.search` и так ходит только в
    /// SearXNG). ДЕФОЛТ false (allowlist-only). Эгресс всё равно через guard: deny_private/SSRF-резолв-
    /// гард/metadata/redirect=none/audit. Эффект при `enabled=true`.
    #[serde(default)]
    pub allow_public_fetch: bool,
}

impl AiConfig {
    /// Дефолт кэпа blast-radius прогона, если не задан в конфиге (консервативный — небольшая пачка
    /// авто-применений до форс-предложения).
    pub const DEFAULT_BLAST_RADIUS_CAP: u32 = 16;

    /// Мин. `wall_clock` прогона (сек): меньше — прогон не успевает даже толком стартовать (cold-start
    /// модели, первый ход) → бессмысленно/опасно. Значения ниже клампятся ВВЕРХ до этого порога.
    pub const MIN_AGENT_WALL_CLOCK_SECS: u64 = 30;
    /// Макс. `wall_clock` (сек, 24 ч): анти-опечатка (прогон длиннее суток — по сути демон, не «прогон»).
    /// Не жёсткий предел продукта — санитарный потолок против `999999999`-подобных ляпов в `local.json`.
    pub const MAX_AGENT_WALL_CLOCK_SECS: u64 = 86_400;
    /// Мин. ходов модели (`0` шагов = прогон без единого хода — бессмысленно). Клампится вверх до 1.
    pub const MIN_AGENT_MAX_STEPS: u64 = 1;
    /// Макс. ходов (анти-опечатка; реальный потолок реального времени всё равно держит `wall_clock`).
    pub const MAX_AGENT_MAX_STEPS: u64 = 10_000;

    /// **BF-1: `wall_clock` прогона агента из конфига с САНИТАРНЫМ клампом** [`MIN_AGENT_WALL_CLOCK_SECS`]
    /// ..[`MAX_AGENT_WALL_CLOCK_SECS`]. `None` (ключа нет / мусорный тип) → `None` — вызыватель берёт
    /// дефолт [`crate::agent::LoopBounds::default`]. Кламп именно здесь (конфиг-слой): невалидно-малое/
    /// большое чинится тихо, а не роняет прогон.
    pub fn agent_wall_clock(&self) -> Option<Duration> {
        self.agent_wall_clock_secs.map(|s| {
            Duration::from_secs(s.clamp(
                Self::MIN_AGENT_WALL_CLOCK_SECS,
                Self::MAX_AGENT_WALL_CLOCK_SECS,
            ))
        })
    }

    /// **BF-1: `max_steps` прогона агента из конфига с клампом** [`MIN_AGENT_MAX_STEPS`]..[`MAX_AGENT_MAX_STEPS`].
    /// `None` → `None` (вызыватель берёт дефолт). `usize::try_from` не может провалиться после клампа к
    /// `MAX_AGENT_MAX_STEPS` (влезает в usize на всех целевых платформах), fallback — тот же потолок.
    pub fn agent_max_steps(&self) -> Option<usize> {
        self.agent_max_steps.map(|s| {
            let clamped = s.clamp(Self::MIN_AGENT_MAX_STEPS, Self::MAX_AGENT_MAX_STEPS);
            usize::try_from(clamped).unwrap_or(Self::MAX_AGENT_MAX_STEPS as usize)
        })
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct ChatConfig {
    pub url: String,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub context_window: Option<usize>,

    // --- INFER-CFG: движок-агностичные таймауты/параметры стрима (все Option, serde-default;
    // отсутствие → встроенный дефолт-геттер → zero-config работает как раньше, но с лучшими
    // дефолтами под cold-start V100). Смена llama.cpp → vLLM (Qwen3.6-27B-AWQ на V100) = только
    // эти поля, без кода. См. `docs/dev/chat.md` (профиль свапа).
    /// Таймаут ПЕРВОГО токена (сек): применяется к инициации стрима И ко всем чанкам ДО первого
    /// полученного байта. Переживает cold-start (V100 компилирует ядра 1–3 мин на первом запросе).
    /// `None` → [`ChatConfig::DEFAULT_FIRST_TOKEN_TIMEOUT_SECS`] (300 с).
    #[serde(default)]
    pub first_token_timeout_secs: Option<u64>,
    /// Idle-таймаут стрима ПОСЛЕ первого байта (сек): детект зависшего стрима в steady-state.
    /// `None` → [`ChatConfig::DEFAULT_IDLE_TIMEOUT_SECS`] (90 с).
    #[serde(default)]
    pub idle_timeout_secs: Option<u64>,
    /// Connect-таймаут TCP-коннекта (сек) у guarded-клиента. `None` →
    /// [`ChatConfig::DEFAULT_CONNECT_TIMEOUT_SECS`] (30 с — безопаснее для V100, ок на LAN).
    #[serde(default)]
    pub connect_timeout_secs: Option<u64>,
    /// Число попыток ИНИЦИАЦИИ запроса (включая первую). `None` →
    /// [`ChatConfig::DEFAULT_RETRY_ATTEMPTS`] (3).
    #[serde(default)]
    pub retry_attempts: Option<u32>,
    /// Температура сэмплинга. `None` → [`ChatConfig::DEFAULT_TEMPERATURE`] (0.3).
    #[serde(default)]
    pub temperature: Option<f32>,
    /// Сколько токенов резервировать под ОТВЕТ модели (вычитается из окна при сборке контекста).
    /// `None` → [`crate::ai::ContextBudget::DEFAULT_RESERVE_OUTPUT`] (1024).
    #[serde(default)]
    pub reserve_output_tokens: Option<usize>,
}

impl ChatConfig {
    /// Дефолт таймаута первого токена (сек) — переживает cold-start крупных моделей на V100.
    pub const DEFAULT_FIRST_TOKEN_TIMEOUT_SECS: u64 = 300;
    /// Дефолт idle-таймаута стрима после первого байта (сек).
    pub const DEFAULT_IDLE_TIMEOUT_SECS: u64 = 90;
    /// Дефолт connect-таймаута (сек).
    pub const DEFAULT_CONNECT_TIMEOUT_SECS: u64 = 30;
    /// Дефолт числа попыток инициации запроса.
    pub const DEFAULT_RETRY_ATTEMPTS: u32 = 3;
    /// Дефолт температуры сэмплинга.
    pub const DEFAULT_TEMPERATURE: f32 = 0.3;

    /// Таймаут первого токена (инициация + чанки ДО первого байта) с дефолтом.
    pub fn first_token_timeout(&self) -> Duration {
        Duration::from_secs(
            self.first_token_timeout_secs
                .unwrap_or(Self::DEFAULT_FIRST_TOKEN_TIMEOUT_SECS),
        )
    }

    /// Idle-таймаут стрима после первого байта с дефолтом.
    pub fn idle_timeout(&self) -> Duration {
        Duration::from_secs(
            self.idle_timeout_secs
                .unwrap_or(Self::DEFAULT_IDLE_TIMEOUT_SECS),
        )
    }

    /// Connect-таймаут с дефолтом (для `GuardedClient::for_chat`).
    pub fn connect_timeout(&self) -> Duration {
        Duration::from_secs(
            self.connect_timeout_secs
                .unwrap_or(Self::DEFAULT_CONNECT_TIMEOUT_SECS),
        )
    }

    /// Число попыток инициации запроса с дефолтом.
    pub fn retry_attempts(&self) -> u32 {
        self.retry_attempts.unwrap_or(Self::DEFAULT_RETRY_ATTEMPTS)
    }

    /// Температура сэмплинга с дефолтом.
    pub fn temperature(&self) -> f32 {
        self.temperature.unwrap_or(Self::DEFAULT_TEMPERATURE)
    }

    /// Резерв токенов под ответ с дефолтом ([`crate::ai::ContextBudget::DEFAULT_RESERVE_OUTPUT`]).
    pub fn reserve_output_tokens(&self) -> usize {
        self.reserve_output_tokens
            .unwrap_or(crate::ai::ContextBudget::DEFAULT_RESERVE_OUTPUT)
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct EmbeddingConfig {
    pub url: String,
    #[serde(default)]
    pub model: Option<String>,
    /// Размерность; если не задана — берётся из ответа модели при первом эмбеддинге.
    #[serde(default)]
    pub dim: Option<usize>,
    /// INFER-CFG: общий таймаут эмбеддинг-запроса (сек) у guarded-клиента (батчи бывают тяжёлые;
    /// V100-профиль ставит больше). `None` → [`EmbeddingConfig::DEFAULT_TIMEOUT_SECS`] (60 с).
    #[serde(default)]
    pub timeout_secs: Option<u64>,
}

impl EmbeddingConfig {
    /// Дефолт таймаута эмбеддинг-запроса (сек).
    pub const DEFAULT_TIMEOUT_SECS: u64 = 60;

    /// Таймаут эмбеддинг-запроса с дефолтом (для `GuardedClient::for_embedding`).
    pub fn timeout(&self) -> Duration {
        Duration::from_secs(self.timeout_secs.unwrap_or(Self::DEFAULT_TIMEOUT_SECS))
    }
}

impl LocalConfig {
    pub fn parse(json: &str) -> AiResult<Self> {
        serde_json::from_str(json).map_err(|e| AiError::Config(e.to_string()))
    }

    /// Хосты явно сконфигурированных `ai.*`-эндпоинтов — для авто-allowlist политики эгресса
    /// (ADR-005-ext E4: «явные `ai.*.url` разрешены», уточнённый AC-SEC-4/E3). Только хост (без
    /// порта/пути) — allowlist exact-host, как у брокера. Невалидные URL пропускаются (провайдер
    /// по ним всё равно не построится; политика — fail-closed).
    pub fn egress_hosts(&self) -> Vec<String> {
        [
            self.ai.chat.as_ref().map(|c| c.url.as_str()),
            self.ai.embedding.as_ref().map(|e| e.url.as_str()),
            self.ai.fast.as_ref().map(|f| f.url.as_str()),
        ]
        .into_iter()
        .flatten()
        .filter_map(|u| {
            reqwest::Url::parse(u)
                .ok()
                .and_then(|u| u.host_str().map(str::to_string))
        })
        .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_local_json() {
        // Форма из ARCHITECTURE §5 (.nexus/local.json).
        let json = r#"{
          "ai": {
            "chat":      { "url": "http://192.168.0.29:8080", "model": "gemma-4-26B-A4B-it", "context_window": 32768 },
            "embedding": { "url": "http://192.168.0.29:8081", "model": "nomic-embed-text", "dim": 768 },
            "reranker":  { "url": "http://192.168.0.29:8082", "enabled": false }
          },
          "sync": { "remote": null }
        }"#;
        let cfg = LocalConfig::parse(json).unwrap();
        let chat = cfg.ai.chat.unwrap();
        assert_eq!(chat.url, "http://192.168.0.29:8080");
        assert_eq!(chat.context_window, Some(32768));
        let emb = cfg.ai.embedding.unwrap();
        assert_eq!(emb.url, "http://192.168.0.29:8081");
        assert_eq!(emb.dim, Some(768));
    }

    #[test]
    fn tolerates_partial_and_unknown_fields() {
        let cfg = LocalConfig::parse(r#"{"ai":{"embedding":{"url":"http://x:8081"}}}"#).unwrap();
        assert!(cfg.ai.chat.is_none());
        assert_eq!(cfg.ai.embedding.unwrap().dim, None);
    }

    #[test]
    fn conn1_absent_connection_is_embedded() {
        // CONN-1: нет `ai.connection` → embedded (нулевая регрессия — агент in-process).
        let cfg = LocalConfig::parse(r#"{"ai":{}}"#).unwrap();
        assert_eq!(cfg.ai.connection.mode(), ConnectionMode::Embedded);
    }

    #[test]
    fn conn1_parses_connection_local() {
        let cfg = LocalConfig::parse(
            r#"{"ai":{"connection":{"mode":"local","socket":"/tmp/agentd.sock"}}}"#,
        )
        .unwrap();
        assert_eq!(cfg.ai.connection.mode(), ConnectionMode::Local);
        assert_eq!(
            cfg.ai.connection.socket.as_deref(),
            Some("/tmp/agentd.sock")
        );
    }

    #[test]
    fn conn1_unknown_mode_falls_back_and_keeps_chat() {
        // КЛЮЧЕВОЙ serde-safety инвариант: мусорный mode НЕ роняет parse и НЕ теряет ai.chat.
        let cfg = LocalConfig::parse(
            r#"{"ai":{"connection":{"mode":"garbage"},"chat":{"url":"http://h:8080"}}}"#,
        )
        .unwrap();
        assert_eq!(cfg.ai.connection.mode(), ConnectionMode::Embedded);
        assert_eq!(cfg.ai.chat.unwrap().url, "http://h:8080");
    }

    #[test]
    fn conn1_wrong_type_mode_does_not_nuke_config() {
        // Ревью CONN-1: НЕВЕРНЫЙ ТИП mode (число/булево/массив) НЕ должен ронять parse и терять ai.chat
        // (голый Option<String> ронял; тип-толерантный десериализатор → None). Тот же класс data-loss.
        for bad in [
            r#"{"ai":{"connection":{"mode":42},"chat":{"url":"http://h:8080"}}}"#,
            r#"{"ai":{"connection":{"mode":true},"chat":{"url":"http://h:8080"}}}"#,
            r#"{"ai":{"connection":{"mode":["x"]},"socket":99,"chat":{"url":"http://h:8080"}}}"#,
        ] {
            let cfg =
                LocalConfig::parse(bad).unwrap_or_else(|e| panic!("parse упал на {bad}: {e}"));
            assert_eq!(cfg.ai.connection.mode(), ConnectionMode::Embedded);
            assert_eq!(
                cfg.ai.chat.expect("ai.chat должен выжить").url,
                "http://h:8080"
            );
        }
    }

    #[test]
    fn acp1_parses_acp_mode_and_command() {
        // on-disk local.json для ai.* — snake_case (ConnectionConfig без rename_all, как и весь AiConfig).
        let cfg = LocalConfig::parse(
            r#"{"ai":{"connection":{"mode":"acp","acp_command":["hermes","acp"],"acp_cwd":"/v"}}}"#,
        )
        .unwrap();
        assert_eq!(cfg.ai.connection.mode(), ConnectionMode::Acp);
        assert_eq!(
            cfg.ai.connection.acp_command.as_deref(),
            Some(["hermes".to_string(), "acp".to_string()].as_slice())
        );
        assert_eq!(cfg.ai.connection.acp_cwd.as_deref(), Some("/v"));
    }

    #[test]
    fn acp1_garbage_acp_command_does_not_nuke_config() {
        // Мусорный acp_command (число / объект / массив-с-не-строками / пустой) → None, parse не падает,
        // ai.chat выживает (та же data-loss-защита).
        for bad in [
            r#"{"ai":{"connection":{"mode":"acp","acp_command":42},"chat":{"url":"http://h:8080"}}}"#,
            r#"{"ai":{"connection":{"mode":"acp","acp_command":{"x":1}},"chat":{"url":"http://h:8080"}}}"#,
            r#"{"ai":{"connection":{"mode":"acp","acp_command":["hermes",7]},"chat":{"url":"http://h:8080"}}}"#,
            r#"{"ai":{"connection":{"mode":"acp","acp_command":[]},"chat":{"url":"http://h:8080"}}}"#,
        ] {
            let cfg =
                LocalConfig::parse(bad).unwrap_or_else(|e| panic!("parse упал на {bad}: {e}"));
            assert_eq!(cfg.ai.connection.mode(), ConnectionMode::Acp);
            assert!(
                cfg.ai.connection.acp_command.is_none(),
                "мусорный acpCommand → None: {bad}"
            );
            assert_eq!(
                cfg.ai.chat.expect("ai.chat должен выжить").url,
                "http://h:8080"
            );
        }
    }

    /// ACP-REMOTE-SSH: `acp_spawn_argv` собирает ssh-команду при `acp_transport="ssh"` (с ключом и без),
    /// падает в `None` при отсутствии host/команды, и откатывается к `acp_command` для local/дефолта.
    #[test]
    fn acp_spawn_argv_resolves_transport() {
        let cfg = |json: &str| LocalConfig::parse(json).unwrap().ai.connection;

        // ssh С ключом: ssh -o … -o … -i key user@host <remote split по пробелам>.
        let ssh_key = cfg(r#"{"ai":{"connection":{"mode":"acp","acp_transport":"ssh",
                 "acp_ssh_host":"artanov@192.168.0.28","acp_ssh_key":"~/.ssh/id_ed25519",
                 "acp_remote_command":"docker exec -i hermes hermes acp"}}}"#);
        assert_eq!(
            ssh_key.acp_spawn_argv().unwrap(),
            vec![
                "ssh",
                "-o",
                "StrictHostKeyChecking=no",
                "-o",
                "BatchMode=yes",
                "-i",
                "~/.ssh/id_ed25519",
                "artanov@192.168.0.28",
                "docker",
                "exec",
                "-i",
                "hermes",
                "hermes",
                "acp"
            ]
        );

        // ssh БЕЗ ключа (пусто/нет) → -i не добавляется (ключ по умолчанию ssh).
        let ssh_nokey = cfg(r#"{"ai":{"connection":{"mode":"acp","acp_transport":"ssh",
                 "acp_ssh_host":"h","acp_ssh_key":"   ","acp_remote_command":"hermes acp"}}}"#);
        assert_eq!(
            ssh_nokey.acp_spawn_argv().unwrap(),
            vec![
                "ssh",
                "-o",
                "StrictHostKeyChecking=no",
                "-o",
                "BatchMode=yes",
                "h",
                "hermes",
                "acp"
            ]
        );

        // ssh без host → None (не сконфигурировано); ssh без remote-команды → None.
        let ssh_no_host = cfg(
            r#"{"ai":{"connection":{"mode":"acp","acp_transport":"ssh","acp_remote_command":"hermes acp"}}}"#,
        );
        assert!(ssh_no_host.acp_spawn_argv().is_none(), "нет host → None");
        let ssh_no_cmd =
            cfg(r#"{"ai":{"connection":{"mode":"acp","acp_transport":"ssh","acp_ssh_host":"h"}}}"#);
        assert!(
            ssh_no_cmd.acp_spawn_argv().is_none(),
            "нет remote-команды → None"
        );

        // local-транспорт → откат к acp_command; ssh-поля игнорируются.
        let local = cfg(
            r#"{"ai":{"connection":{"mode":"acp","acp_transport":"local","acp_command":["hermes","acp"],
                 "acp_ssh_host":"unused"}}}"#,
        );
        assert_eq!(
            local.acp_spawn_argv().unwrap(),
            vec!["hermes".to_string(), "acp".into()]
        );

        // Отсутствие transport (None) → дефолт local → acp_command.
        let default_tr =
            cfg(r#"{"ai":{"connection":{"mode":"acp","acp_command":["hermes","acp"]}}}"#);
        assert_eq!(
            default_tr.acp_spawn_argv().unwrap(),
            vec!["hermes".to_string(), "acp".into()]
        );

        // local без acp_command → None.
        let local_empty = cfg(r#"{"ai":{"connection":{"mode":"acp","acp_transport":"local"}}}"#);
        assert!(
            local_empty.acp_spawn_argv().is_none(),
            "local без команды → None"
        );
    }

    /// ACP-REMOTE-SSH: 4 новых поля round-trip'ятся через JSON (snake_case), и мусорный тип любого из них
    /// не роняет parse / не теряет ai.chat (та же data-loss-защита, что у acp_command).
    #[test]
    fn acp_ssh_fields_round_trip_snake_case() {
        let cfg = LocalConfig::parse(
            r#"{"ai":{"connection":{"mode":"acp","acp_transport":"ssh",
                 "acp_ssh_host":"artanov@192.168.0.28","acp_ssh_key":"~/.ssh/id_ed25519",
                 "acp_remote_command":"docker exec -i hermes hermes acp"}}}"#,
        )
        .unwrap()
        .ai
        .connection;
        assert_eq!(cfg.acp_transport.as_deref(), Some("ssh"));
        assert_eq!(cfg.acp_ssh_host.as_deref(), Some("artanov@192.168.0.28"));
        assert_eq!(cfg.acp_ssh_key.as_deref(), Some("~/.ssh/id_ed25519"));
        assert_eq!(
            cfg.acp_remote_command.as_deref(),
            Some("docker exec -i hermes hermes acp")
        );

        // Мусорный тип (число/массив/объект) → None, parse не падает, ai.chat выживает.
        let bad = LocalConfig::parse(
            r#"{"ai":{"connection":{"mode":"acp","acp_transport":42,"acp_ssh_host":["x"],
                 "acp_ssh_key":{"k":1},"acp_remote_command":true},"chat":{"url":"http://h:8080"}}}"#,
        )
        .unwrap();
        assert!(bad.ai.connection.acp_transport.is_none());
        assert!(bad.ai.connection.acp_ssh_host.is_none());
        assert!(bad.ai.connection.acp_ssh_key.is_none());
        assert!(bad.ai.connection.acp_remote_command.is_none());
        assert_eq!(bad.ai.chat.expect("ai.chat выживает").url, "http://h:8080");
    }

    /// E4: авто-allowlist берёт ИМЕННО хосты явных `ai.*.url` (chat/embedding/fast), без порта;
    /// битый URL пропускается, пустой конфиг → пусто (fail-closed).
    #[test]
    fn egress_hosts_extracts_explicit_ai_hosts() {
        let cfg = LocalConfig::parse(
            r#"{"ai":{
                "chat":      { "url": "https://api.example.com/v1" },
                "embedding": { "url": "http://192.168.0.29:8083" },
                "fast":      { "url": "not a url" }
            }}"#,
        )
        .unwrap();
        let hosts = cfg.egress_hosts();
        assert_eq!(
            hosts,
            vec!["api.example.com".to_string(), "192.168.0.29".to_string()]
        );
        assert!(LocalConfig::default().egress_hosts().is_empty());
    }

    /// P0-c: `ai.tokenizer_path` парсится (смена модели токенайзера = файл+конфиг); по умолчанию None.
    #[test]
    fn parses_tokenizer_path() {
        let cfg = LocalConfig::parse(r#"{"ai":{"tokenizer_path":"/vault/.nexus/tokenizer.json"}}"#)
            .unwrap();
        assert_eq!(
            cfg.ai.tokenizer_path.as_deref(),
            Some("/vault/.nexus/tokenizer.json")
        );
        // Нет ключа → None (встроенный токенайзер задеплоенной модели).
        assert!(LocalConfig::parse(r#"{"ai":{}}"#)
            .unwrap()
            .ai
            .tokenizer_path
            .is_none());
    }

    /// AGENT-3e SAFE-BY-DEFAULT: флаг актуатора по умолчанию FALSE (нет ключа → без инструментов
    /// записи, реальный vault не затронут). Связанные пороги по умолчанию None (берётся ядровый
    /// дефолт). Включается явно.
    #[test]
    fn agent_actuator_disabled_by_default() {
        // Пустой ai-блок → флаг false, пороги None.
        let cfg = LocalConfig::parse(r#"{"ai":{}}"#).unwrap();
        assert!(
            !cfg.ai.agent_actuator_enabled,
            "актуатор ВЫКЛ по умолчанию (safe-by-default)"
        );
        assert!(cfg.ai.agent_overwrite_threshold.is_none());
        assert!(cfg.ai.agent_blast_radius_cap.is_none());

        // Полностью пустой конфиг → тоже false.
        assert!(!LocalConfig::default().ai.agent_actuator_enabled);

        // Явный opt-in + пороги читаются.
        let on = LocalConfig::parse(
            r#"{"ai":{"agent_actuator_enabled":true,"agent_overwrite_threshold":4096,"agent_blast_radius_cap":4}}"#,
        )
        .unwrap();
        assert!(on.ai.agent_actuator_enabled);
        assert_eq!(on.ai.agent_overwrite_threshold, Some(4096));
        assert_eq!(on.ai.agent_blast_radius_cap, Some(4));
    }

    /// SANDBOX-6c-3d: `ai.git_worktree` (owner-gated undo-worktree для реального exec-GitOp reset) по
    /// умолчанию None (откат остаётся Deferred); парсится явно. Vault всегда `:ro` — это ОТДЕЛЬНЫЙ rw-mount.
    #[test]
    fn git_worktree_default_none_and_parses() {
        assert!(
            LocalConfig::parse(r#"{"ai":{}}"#)
                .unwrap()
                .ai
                .git_worktree
                .is_none(),
            "git_worktree None по умолчанию (undo Deferred, safe)"
        );
        let on = LocalConfig::parse(r#"{"ai":{"git_worktree":"/srv/sbx-repo"}}"#).unwrap();
        assert_eq!(on.ai.git_worktree.as_deref(), Some("/srv/sbx-repo"));
    }

    /// SKILL-2: `agent_skills_dir` по умолчанию None (агент без скиллов, без регрессии); парсится явно.
    #[test]
    fn parses_agent_skills_dir() {
        assert!(LocalConfig::parse(r#"{"ai":{}}"#)
            .unwrap()
            .ai
            .agent_skills_dir
            .is_none());
        let on =
            LocalConfig::parse(r#"{"ai":{"agent_skills_dir":"/vault/.nexus/skills"}}"#).unwrap();
        assert_eq!(
            on.ai.agent_skills_dir.as_deref(),
            Some("/vault/.nexus/skills")
        );
    }

    /// INFER-CFG: новые поля инференса. Zero-config → дефолты через геттеры (обратная совместимость);
    /// явные значения парсятся. Дефолты: first_token 300с (cold-start V100), idle 90с, connect 30с,
    /// retry 3, temperature 0.3, embedding-timeout 60с.
    #[test]
    fn infer_cfg_timeouts_defaults_and_overrides() {
        // Zero-config: chat-секция без новых полей → геттеры дают дефолты.
        let zc = LocalConfig::parse(r#"{"ai":{"chat":{"url":"http://h:8080"}}}"#).unwrap();
        let c = zc.ai.chat.unwrap();
        assert_eq!(c.first_token_timeout(), Duration::from_secs(300));
        assert_eq!(c.idle_timeout(), Duration::from_secs(90));
        assert_eq!(c.connect_timeout(), Duration::from_secs(30));
        assert_eq!(c.retry_attempts(), 3);
        assert!((c.temperature() - 0.3).abs() < f32::EPSILON);
        // Embedding zero-config → дефолтный таймаут.
        let ze = LocalConfig::parse(r#"{"ai":{"embedding":{"url":"http://h:8081"}}}"#).unwrap();
        assert_eq!(ze.ai.embedding.unwrap().timeout(), Duration::from_secs(60));

        // Явные значения (целевой 1Cat-vLLM/V100 профиль) — уважаются геттерами.
        let oc = LocalConfig::parse(
            r#"{"ai":{"chat":{"url":"http://h:8000","model":"qwen3.6-27b-awq-mtp","context_window":262144,
                 "first_token_timeout_secs":240,"idle_timeout_secs":120,"connect_timeout_secs":45,
                 "retry_attempts":1,"temperature":0.7,"reserve_output_tokens":2048},
                 "embedding":{"url":"http://h:8001","timeout_secs":180}}}"#,
        )
        .unwrap();
        let c = oc.ai.chat.unwrap();
        assert_eq!(c.first_token_timeout(), Duration::from_secs(240));
        assert_eq!(c.idle_timeout(), Duration::from_secs(120));
        assert_eq!(c.connect_timeout(), Duration::from_secs(45));
        assert_eq!(c.retry_attempts(), 1);
        assert!((c.temperature() - 0.7).abs() < f32::EPSILON);
        assert_eq!(c.reserve_output_tokens(), 2048);
        assert_eq!(c.context_window, Some(262144));
        assert_eq!(oc.ai.embedding.unwrap().timeout(), Duration::from_secs(180));
    }

    #[test]
    fn parses_fast_utility_endpoint() {
        let cfg = LocalConfig::parse(
            r#"{"ai":{"fast":{"url":"http://192.168.0.29:8084","model":"qwen"}}}"#,
        )
        .unwrap();
        let fast = cfg.ai.fast.unwrap();
        assert_eq!(fast.url, "http://192.168.0.29:8084");
        assert_eq!(fast.model.as_deref(), Some("qwen"));
        // Нет секции fast → None (fallback на gemma-fast в open_vault).
        assert!(LocalConfig::parse(r#"{"ai":{}}"#)
            .unwrap()
            .ai
            .fast
            .is_none());
    }

    /// Agent-флаги настроек (агентд-only): `agent_autonomy`/`sandbox_enabled`/`shell_enable` —
    /// дефолты SAFE (None/false), явные значения парсятся. Эти поля выводятся тогглами Настроек→ИИ
    /// в `local.json`, читаются headless-агентом (`nexus-agentd`).
    #[test]
    fn parses_agent_runtime_flags() {
        // Пусто → дефолты safe.
        let zc = LocalConfig::parse(r#"{"ai":{}}"#).unwrap();
        assert!(zc.ai.agent_autonomy.is_none(), "autonomy None → confirm");
        assert!(!zc.ai.sandbox_enabled);
        assert!(!zc.ai.shell_enable);

        // Явный opt-in.
        let on = LocalConfig::parse(
            r#"{"ai":{"agent_autonomy":"auto","sandbox_enabled":true,"shell_enable":true}}"#,
        )
        .unwrap();
        assert_eq!(on.ai.agent_autonomy.as_deref(), Some("auto"));
        assert!(on.ai.sandbox_enabled);
        assert!(on.ai.shell_enable);
    }

    /// SAFETY (тоггл `allow_public_fetch` в Настройках): частичный `ai.web` БЕЗ `url` (тоггл пишет лишь
    /// `allow_public_fetch`, а `url`/`enabled` живут в отдельном `websearch.json` десктопа) обязан
    /// ПАРСИТЬСЯ — иначе `WebConfig.url` без `#[serde(default)]` уронил бы весь `local.json`
    /// (потеря chat/embedding-конфига). url пуст → веб ИНЕРТЕН (агентд требует `enabled && !url.empty`).
    #[test]
    fn partial_web_config_with_only_public_fetch_parses_and_is_inert() {
        let cfg = LocalConfig::parse(r#"{"ai":{"web":{"allow_public_fetch":true}}}"#).unwrap();
        let web = cfg.ai.web.expect("ai.web парсится без url (serde default)");
        assert!(web.url.is_empty(), "url по умолчанию пуст");
        assert!(!web.enabled, "enabled по умолчанию false → веб инертен");
        assert!(web.allow_public_fetch, "флаг прочитан");

        // chat/embedding в том же документе НЕ теряются (раньше парс падал бы целиком).
        let mixed = LocalConfig::parse(
            r#"{"ai":{"chat":{"url":"http://h:8080"},"web":{"allow_public_fetch":true}}}"#,
        )
        .unwrap();
        assert_eq!(mixed.ai.chat.unwrap().url, "http://h:8080");
        assert!(mixed.ai.web.unwrap().allow_public_fetch);
    }

    /// SL-7: `ai.skills.learning_enabled` — дефолт false (NON-Option SkillsConfig, нет `ai.skills` → all
    /// false), явный opt-in парсится, и частичный `ai.skills` не роняет соседний chat/embedding-конфиг.
    #[test]
    fn parses_skills_learning_flag() {
        // Пусто → дефолт OFF.
        let zc = LocalConfig::parse(r#"{"ai":{}}"#).unwrap();
        assert!(
            !zc.ai.skills.learning_enabled,
            "нет ai.skills → learning_enabled false (safe default)"
        );

        // Явный opt-in.
        let on = LocalConfig::parse(r#"{"ai":{"skills":{"learning_enabled":true}}}"#).unwrap();
        assert!(on.ai.skills.learning_enabled, "явный true прочитан");

        // Частичный ai.skills рядом с chat — оба сохраняются (serde default не роняет документ).
        let mixed = LocalConfig::parse(
            r#"{"ai":{"chat":{"url":"http://h:8080"},"skills":{"learning_enabled":true}}}"#,
        )
        .unwrap();
        assert_eq!(mixed.ai.chat.unwrap().url, "http://h:8080");
        assert!(mixed.ai.skills.learning_enabled);
    }

    /// SUB-0: `ai.delegation` дефолт-OFF + консервативные капы; частичный конфиг падает в дефолты полей.
    #[test]
    fn delegation_config_defaults_off() {
        // Пусто → дефолт OFF + капы hermes-tuned.
        let zc = LocalConfig::parse(r#"{"ai":{}}"#).unwrap();
        assert!(
            !zc.ai.delegation.enabled,
            "нет ai.delegation → enabled false"
        );
        assert_eq!(zc.ai.delegation.max_depth, 1);
        assert_eq!(zc.ai.delegation.max_fanout, 3);
        assert_eq!(zc.ai.delegation.max_total_spawns, 8);

        // Явный opt-in + частичная секция: enabled читается, незаданные капы — дефолты (не 0).
        let on =
            LocalConfig::parse(r#"{"ai":{"delegation":{"enabled":true,"max_fanout":5}}}"#).unwrap();
        assert!(on.ai.delegation.enabled);
        assert_eq!(on.ai.delegation.max_fanout, 5, "явный кап прочитан");
        assert_eq!(
            on.ai.delegation.max_depth, 1,
            "незаданный кап → дефолт, не 0"
        );
        assert_eq!(on.ai.delegation.max_total_spawns, 8);
    }

    /// RES-1: `ai.research` дефолт-OFF + консервативные капы; частичный конфиг падает в дефолты полей.
    #[test]
    fn research_config_defaults_off() {
        let zc = LocalConfig::parse(r#"{"ai":{}}"#).unwrap();
        assert!(!zc.ai.research.enabled, "нет ai.research → enabled false");
        assert_eq!(zc.ai.research.max_rounds, 3);
        assert_eq!(zc.ai.research.max_urls_per_round, 3);
        assert_eq!(zc.ai.research.max_content_chars, 15000);
        assert_eq!(zc.ai.research.extraction_concurrency, 3);
        assert_eq!(zc.ai.research.wall_clock_secs, 0);

        // Явный opt-in + частичная секция: enabled читается, незаданные капы — дефолты (не 0).
        let on =
            LocalConfig::parse(r#"{"ai":{"research":{"enabled":true,"max_rounds":5}}}"#).unwrap();
        assert!(on.ai.research.enabled);
        assert_eq!(on.ai.research.max_rounds, 5, "явный кап прочитан");
        assert_eq!(
            on.ai.research.max_urls_per_round, 3,
            "незаданный кап → дефолт, не 0"
        );
    }

    /// BF-1: `ai.agent_wall_clock_secs`/`ai.agent_max_steps` — по умолчанию None (вызыватель берёт
    /// `LoopBounds::default`); валидные значения парсятся; геттеры КЛАМПЯТ [30..86400] с и [1..10000] шагов.
    #[test]
    fn bf1_agent_bounds_parse_clamp_and_defaults() {
        // Нет ключей → геттеры None (дефолтный путь; байт-прежнее поведение у вызывателя).
        let zc = LocalConfig::parse(r#"{"ai":{}}"#).unwrap();
        assert!(zc.ai.agent_wall_clock_secs.is_none());
        assert!(zc.ai.agent_max_steps.is_none());
        assert!(zc.ai.agent_wall_clock().is_none());
        assert!(zc.ai.agent_max_steps().is_none());

        // Валидные значения в допустимом диапазоне — проходят как есть.
        let ok = LocalConfig::parse(r#"{"ai":{"agent_wall_clock_secs":600,"agent_max_steps":20}}"#)
            .unwrap();
        assert_eq!(ok.ai.agent_wall_clock(), Some(Duration::from_secs(600)));
        assert_eq!(ok.ai.agent_max_steps(), Some(20));

        // Кламп ВНИЗ: слишком малый wall_clock (5с) → 30с; 0 шагов → 1.
        let lo = LocalConfig::parse(r#"{"ai":{"agent_wall_clock_secs":5,"agent_max_steps":0}}"#)
            .unwrap();
        assert_eq!(
            lo.ai.agent_wall_clock(),
            Some(Duration::from_secs(AiConfig::MIN_AGENT_WALL_CLOCK_SECS))
        );
        assert_eq!(lo.ai.agent_max_steps(), Some(1));

        // Кламп ВВЕРХ: абсурдно большие значения → потолки (анти-опечатка).
        let hi = LocalConfig::parse(
            r#"{"ai":{"agent_wall_clock_secs":999999999,"agent_max_steps":999999999}}"#,
        )
        .unwrap();
        assert_eq!(
            hi.ai.agent_wall_clock(),
            Some(Duration::from_secs(AiConfig::MAX_AGENT_WALL_CLOCK_SECS))
        );
        assert_eq!(
            hi.ai.agent_max_steps(),
            Some(AiConfig::MAX_AGENT_MAX_STEPS as usize)
        );
    }

    /// BF-1 (data-loss-защита, прецедент CONN-1): МУСОРНЫЙ тип у `agent_wall_clock_secs`/`agent_max_steps`
    /// — строка `"42"`, булево, массив, объект, отрицательное, дробное — НЕ роняет `LocalConfig::parse` и
    /// НЕ теряет `ai.chat`: значение просто → `None` (→ дефолт у вызывателя). Голый `Option<u64>` ронял бы.
    #[test]
    fn bf1_agent_bounds_tolerant_garbage_survives() {
        for bad in [
            r#"{"ai":{"agent_wall_clock_secs":"42","chat":{"url":"http://h:8080"}}}"#,
            r#"{"ai":{"agent_wall_clock_secs":true,"chat":{"url":"http://h:8080"}}}"#,
            r#"{"ai":{"agent_wall_clock_secs":[42],"chat":{"url":"http://h:8080"}}}"#,
            r#"{"ai":{"agent_wall_clock_secs":{"x":1},"chat":{"url":"http://h:8080"}}}"#,
            r#"{"ai":{"agent_wall_clock_secs":-5,"chat":{"url":"http://h:8080"}}}"#,
            r#"{"ai":{"agent_wall_clock_secs":30.5,"chat":{"url":"http://h:8080"}}}"#,
            r#"{"ai":{"agent_max_steps":"8","chat":{"url":"http://h:8080"}}}"#,
            r#"{"ai":{"agent_max_steps":["x"],"chat":{"url":"http://h:8080"}}}"#,
        ] {
            let cfg =
                LocalConfig::parse(bad).unwrap_or_else(|e| panic!("parse упал на {bad}: {e}"));
            // Ревью R-13g-волны: per-key ассерты (не `||`) — иначе кейсы max_steps «страховались бы»
            // тривиальным None соседнего отсутствующего ключа и мутант «нетолерантный max_steps» выживал.
            if bad.contains("agent_wall_clock_secs") {
                assert!(
                    cfg.ai.agent_wall_clock().is_none(),
                    "мусорный wall_clock → None: {bad}"
                );
            } else {
                assert!(
                    cfg.ai.agent_max_steps().is_none(),
                    "мусорный max_steps → None: {bad}"
                );
            }
            assert_eq!(
                cfg.ai.chat.expect("ai.chat должен выжить").url,
                "http://h:8080",
                "мусорный agent-бюджет не роняет ai.chat: {bad}"
            );
        }

        // Контроль: голое JSON-число ПРИНИМАЕТСЯ (не спутать «толерантность» с «игнорирую всё»).
        let good =
            LocalConfig::parse(r#"{"ai":{"agent_wall_clock_secs":42,"agent_max_steps":42}}"#)
                .unwrap();
        assert_eq!(good.ai.agent_wall_clock_secs, Some(42));
        assert_eq!(good.ai.agent_max_steps, Some(42));
    }
}
