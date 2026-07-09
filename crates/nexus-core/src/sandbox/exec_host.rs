//! host/exec — RPC исполнения Фаза-3 host exec-таргетов (SANDBOX-6c, спека §5.2).
//!
//! **КЛЮЧЕВАЯ ИНВЕРСИЯ §5.2:** РЕШЕНИЕ host-side (classify→approval→ledger), ИСПОЛНЕНИЕ — ВНУТРИ
//! `--network=none` контейнера (6c-2 `exec_child`). host НИКОГДА не запускает команду (jailbroken
//! `rm -rf` упирается в EROFS/ENETUNREACH/cap-deny на УРОВНЕ ЯДРА песочницы, а не в Rust-if хоста с
//! полными правами). `host/exec` — ВТОРОЙ метод на act.sock (НЕ 4-й сокет): `WireKind` несёт лишь 3
//! vault-вида, `WireExecKind` — лишь 3 exec-вида → forge невозможен by-construction в обе стороны.
//!
//! 2-фазный (3 request/response) поток, КАЖДЫЙ инициирует КОНТЕЙНЕР (host только отвечает):
//!  - **decide**: контейнер шлёт `{phase:decide, action}` → host `dispatch_exec_decision` (classify_exec →
//!    HardBlocked/Confirm; PolicyDefault Confirm=DENY; коннектор Confirm=Proposal→`agent/approve`). На
//!    Approve: ledger PROPOSED→APPROVED + СОХРАНЯЕТ Action host-side + минтит одноразовый `exec_token`,
//!    привязанный к (run_id, ledger action_id, fingerprint действия). Ответ Approved/Rejected/HardBlocked.
//!  - **execute** (6c-2, только на approved): контейнер шлёт `{phase:execute, exec_token}` → host
//!    валидирует+КОНСЬЮМИТ токен (одноразовый), ledger APPROVED→EXECUTING, возвращает `WireExecGo` с
//!    host-нормализованным argv (контейнер argv НЕ переподаёт → закрыт TOCTOU approve-ls-run-rm).
//!  - **report** (6c-2): контейнер шлёт исход → host финализирует ledger EXECUTED/FAILED.
//!
//! **6c-2d (текущий уровень):** реализован ВЕСЬ host-цикл — `decide` (6c-1) + `execute` (redeem токена →
//! ledger `APPROVED→EXECUTING` + host-authority `WireExecGo`, kill-switch last-moment, 6c-2c) + `report`
//! (консьюм `in_flight` → ledger `EXECUTING→EXECUTED|FAILED`, СТРУКТУРНЫЙ outcome БЕЗ сырого вывода).
//! `HostExecServer` роутит все 3 фазы. Контейнерный исполнитель (`exec_child` — есть, 6c-2a) подключается
//! ProxyExec-шимом + `serve_host`-проводкой (6c-2e/2f). host НИКОГДА не исполняет команду здесь.
//!
//! **6c-2 ОБЯЗАН (review hard-gates, флагнуто здесь чтобы инвариант не дрейфанул):**
//!  1. ✅ `redeem` (execute, 6c-2c) КОНСЬЮМИТ токен из `pending` (одноразовость + кэп [`MAX_PENDING_EXEC`]
//!     на `in_flight` симметрично `pending`); 6c-2d `report` КОНСЬЮМИТ `in_flight` (финализация);
//!  2. `WireExecGo.env` строится ТОЛЬКО из env-allowlist (спека §5.4, ✅ `build_exec_env` 6c-2b), НЕ из
//!     host-env — НИ ОДНОГО секрета;
//!  3. `serve_exec`/`serve_host` оборачивает accept-путь в `peer_authorized` (`SO_PEERCRED`), как
//!     serve_act/egress/event — иначе любой локальный процесс гонял бы decide/redeem (6c-2f).

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::actuator::audit::{
    self, UndoCols, STATE_APPROVED, STATE_EXECUTED, STATE_EXECUTING, STATE_FAILED,
};
use crate::actuator::{Action, ActionTarget, UNDO_EXEC_GITREF};
use crate::agent::connect::RpcError;

/// JSON-RPC метод (на act.sock, ВТОРОЙ после `host/act`): исполнение exec-таргета.
pub const HOST_EXEC: &str = "host/exec";

/// Вид exec-таргета на проводе (ТОЛЬКО exec — зеркало-противоположность [`super::act::WireKind`], который
/// несёт лишь vault-виды). 3 вида → контейнер не выразит vault через host/exec и наоборот.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WireExecKind {
    ShellRun,
    ProcessSpawn,
    GitOp,
}

/// Wire-DTO exec-действия (≠ `actuator::Action`). Плоский (`flatten`+`deny_unknown_fields` конфликтуют):
/// поля по виду опциональны. `deny_unknown_fields` — fail-closed (лишнее поле → отказ).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WireExecAction {
    pub kind: WireExecKind,
    /// argv (ShellRun).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub argv: Vec<String>,
    /// Программа (ProcessSpawn).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub program: Option<String>,
    /// Аргументы (ProcessSpawn / GitOp).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    /// Git-операция (GitOp).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub op: Option<String>,
    /// Рабочий каталог (vault-rel; ShellRun/ProcessSpawn).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd_rel: Option<String>,
}

impl TryFrom<&Action> for WireExecAction {
    type Error = &'static str;
    /// FAIL-CLOSED: vault-таргеты НЕ представимы на host/exec → `Err` (их путь — `host/act`). EXHAUSTIVE
    /// (без `_ =>`): новый ActionTarget-вариант осознанно решит, exec он или vault.
    fn try_from(a: &Action) -> Result<Self, Self::Error> {
        match &a.target {
            ActionTarget::NoteCreate { .. }
            | ActionTarget::NoteEdit { .. }
            | ActionTarget::Frontmatter { .. } => {
                Err("vault-таргет не представим на host/exec (используй host/act)")
            }
            // SL-7: SkillSave — файловая запись (не exec) → на host/exec непредставим (его путь — отдельный
            // in-process apply_skill_save; на sandbox-wire v1 не выносится).
            ActionTarget::SkillSave { .. } => {
                Err("SkillSave не представим на host/exec (это файловая запись, не команда)")
            }
            ActionTarget::ShellRun { argv, cwd_rel } => Ok(WireExecAction {
                kind: WireExecKind::ShellRun,
                argv: argv.clone(),
                program: None,
                args: Vec::new(),
                op: None,
                cwd_rel: cwd_rel.clone(),
            }),
            ActionTarget::ProcessSpawn {
                program,
                args,
                cwd_rel,
            } => Ok(WireExecAction {
                kind: WireExecKind::ProcessSpawn,
                argv: Vec::new(),
                program: Some(program.clone()),
                args: args.clone(),
                op: None,
                cwd_rel: cwd_rel.clone(),
            }),
            ActionTarget::GitOp { op, args } => Ok(WireExecAction {
                kind: WireExecKind::GitOp,
                argv: Vec::new(),
                program: None,
                args: args.clone(),
                op: Some(op.clone()),
                cwd_rel: None,
            }),
        }
    }
}

impl TryFrom<WireExecAction> for Action {
    type Error = &'static str;
    fn try_from(w: WireExecAction) -> Result<Self, Self::Error> {
        let target = match w.kind {
            WireExecKind::ShellRun => ActionTarget::ShellRun {
                argv: w.argv,
                cwd_rel: w.cwd_rel,
            },
            WireExecKind::ProcessSpawn => ActionTarget::ProcessSpawn {
                program: w.program.ok_or("process_spawn требует program")?,
                args: w.args,
                cwd_rel: w.cwd_rel,
            },
            WireExecKind::GitOp => ActionTarget::GitOp {
                op: w.op.ok_or("git_op требует op")?,
                args: w.args,
            },
        };
        Ok(Action {
            target,
            content: None,
            value: None,
        })
    }
}

/// Фаза 3-актного host/exec (плоский дискриминант). Поля запроса опциональны по фазе.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WireExecPhase {
    Decide,
    Execute,
    Report,
}

/// Запрос host/exec (`deny_unknown_fields`, плоский). `action` — только decide; `exec_token` — execute/
/// report (там ОБЯЗАТЕЛЕН); exit/tails/undo_ref — только report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WireExecRequest {
    pub phase: WireExecPhase,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<WireExecAction>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exec_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stdout_tail: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stderr_tail: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub undo_ref: Option<String>,
}

/// Ответ фазы `decide`. Approved несёт ОДНОРАЗОВЫЙ `exec_token` (привязан host-side к действию+ledger-id)
/// — контейнер предъявляет ТОЛЬКО его на `execute` (argv не переподаёт → нет approve-ls-run-rm).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum WireExecDecision {
    Approved {
        exec_token: String,
        ledger_action_id: i64,
    },
    Rejected {
        summary: String,
    },
    HardBlocked {
        reason: String,
    },
}

/// Рабочий каталог исполнения в контейнере (6c-2). ScratchTmpfs — writable per-run tmpfs; VaultRo — `:ro`
/// vault (запись → EROFS).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ExecCwd {
    ScratchTmpfs { rel: String },
    VaultRo { rel: String },
}

/// Сигнал «исполни» (фаза execute, 6c-2): host-нормализованный argv (БЕЗ шелла) + cwd + ПОЛНЫЙ env-набор
/// (контейнер делает `env_clear()`+это) + кэпы. argv строит HOST из СОХРАНЁННОГО действия — контейнер не
/// переподаёт (TOCTOU-замок).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WireExecGo {
    pub argv: Vec<String>,
    pub cwd: ExecCwd,
    pub env: Vec<(String, String)>,
    pub timeout_ms: u64,
    pub output_cap_bytes: usize,
}

/// Ответ фазы `report` (6c-2): зафиксирован ли исход в ledger.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WireExecResult {
    pub exit_code: i32,
    pub finalized: bool,
}

/// Host-side абстракция exec-actuator (за ней — `dispatch_exec_decision` + token-store + ledger). Вынесена
/// ради Tier-1-тестируемости `HostExecServer` (мок). Реальный — `DispatchExecBackend` (6c-1 decide;
/// 6c-2 redeem/finalize).
#[async_trait]
pub trait ExecBackend: Send + Sync {
    /// Фаза decide: классифицировать+решить (host-side). Approve → СОХРАНИТЬ действие + минт токена.
    async fn decide(&self, action: &Action) -> WireExecDecision;

    /// Фаза execute (6c-2c): redeem ОДНОРАЗОВОГО `exec_token` → host-нормализованный [`WireExecGo`]
    /// (argv из СОХРАНЁННОГО действия — контейнер не переподаёт). По умолчанию `invalid_params` (мок/
    /// 6c-1-уровень не исполняет); реальный — [`DispatchExecBackend`]. Ошибка → `RpcError` (неизвестный/
    /// консьюмнутый токен, пауза, гонка ledger).
    async fn execute(&self, _exec_token: &str) -> Result<WireExecGo, RpcError> {
        Err(RpcError::invalid_params())
    }

    /// Фаза report (6c-2d): финализация исхода исполнения. КОНСЬЮМИТ `in_flight[token]` → ledger
    /// `EXECUTING→EXECUTED|FAILED`. **Приватность**: в ledger пишется СТРУКТУРНОЕ резюме (exit + байт-
    /// счётчики), НЕ сырой stdout/stderr. `undo_ref` принимается на проводе, но не персистится (→6c-2h).
    /// По умолчанию `invalid_params` (мок/6c-1-уровень). Ошибка → `RpcError` (нет in_flight / replay / гонка).
    async fn report(
        &self,
        _exec_token: &str,
        _exit_code: i32,
        _stdout_tail: &str,
        _stderr_tail: &str,
        _undo_ref: Option<&str>,
    ) -> Result<WireExecResult, RpcError> {
        Err(RpcError::invalid_params())
    }
}

/// Host-обработчик `host/exec`. 6c-2c: маршрутизирует `decide`+`execute` (redeem) → `backend`;
/// `report` зарезервирован (6c-2d) → `invalid_params` (fail-closed, если кто-то пошлёт раньше времени).
pub struct HostExecServer<B: ExecBackend> {
    backend: B,
}

impl<B: ExecBackend> HostExecServer<B> {
    pub fn new(backend: B) -> Self {
        Self { backend }
    }

    /// Обрабатывает один `host/exec`-запрос. `Ok(Value)` = сериализованный фазовый ответ.
    pub async fn handle(&self, method: &str, params: Value) -> Result<Value, RpcError> {
        if method != HOST_EXEC {
            return Err(RpcError::method_not_found());
        }
        let req: WireExecRequest =
            serde_json::from_value(params).map_err(|_| RpcError::invalid_params())?;
        match req.phase {
            WireExecPhase::Decide => {
                // fail-closed: decide несёт ТОЛЬКО {phase, action}. execute/report-поля в decide-запросе —
                // отказ (`deny_unknown_fields` ловит лишь НЕИЗВЕСТНЫЕ struct-поля, не КРОСС-ФАЗОВЫЕ известные).
                // Закрывает щель, где спутанный/злонамеренный клиент протолкнул бы exec_token/exit/tails в decide.
                if req.exec_token.is_some()
                    || req.exit_code.is_some()
                    || req.stdout_tail.is_some()
                    || req.stderr_tail.is_some()
                    || req.undo_ref.is_some()
                {
                    return Err(RpcError::invalid_params());
                }
                let wire = req.action.ok_or_else(RpcError::invalid_params)?;
                // exec-only by-construction: WireExecAction→Action даёт лишь exec-таргеты.
                let action: Action = wire.try_into().map_err(|_| RpcError::invalid_params())?;
                let decision = self.backend.decide(&action).await;
                serde_json::to_value(decision).map_err(|e| RpcError::internal(e.to_string()))
            }
            WireExecPhase::Execute => {
                // fail-closed: execute несёт ТОЛЬКО exec_token (decide/report-поля → отказ, кросс-фаза).
                if req.action.is_some()
                    || req.exit_code.is_some()
                    || req.stdout_tail.is_some()
                    || req.stderr_tail.is_some()
                    || req.undo_ref.is_some()
                {
                    return Err(RpcError::invalid_params());
                }
                let token = req.exec_token.ok_or_else(RpcError::invalid_params)?;
                let go = self.backend.execute(&token).await?;
                serde_json::to_value(go).map_err(|e| RpcError::internal(e.to_string()))
            }
            WireExecPhase::Report => {
                // fail-closed: report несёт exec_token + exit_code (+ tails/undo_ref); `action` → отказ.
                if req.action.is_some() {
                    return Err(RpcError::invalid_params());
                }
                let token = req.exec_token.ok_or_else(RpcError::invalid_params)?;
                let exit_code = req.exit_code.ok_or_else(RpcError::invalid_params)?;
                let stdout = req.stdout_tail.unwrap_or_default();
                let stderr = req.stderr_tail.unwrap_or_default();
                let result = self
                    .backend
                    .report(&token, exit_code, &stdout, &stderr, req.undo_ref.as_deref())
                    .await?;
                serde_json::to_value(result).map_err(|e| RpcError::internal(e.to_string()))
            }
        }
    }
}

/// Запомненное одобренное exec-действие. Контейнер на `execute` (6c-2c) предъявит ТОЛЬКО `exec_token`;
/// host строит `WireExecGo` argv из ЭТОГО сохранённого действия (контейнер argv не переподаёт → TOCTOU-
/// замок approve-`ls`-run-`rm`). `propose_key` — idempotency-ключ ledger-строки (redeem/finalize фенсят
/// `approved→executing→executed|failed` по нему). Консьюмится execute (redeem, 6c-2c).
struct PendingExec {
    action: Action,
    ledger_action_id: i64,
    propose_key: String,
}

/// Висящее ИСПОЛНЯЕМОЕ exec-действие (после redeem, ledger=EXECUTING). report (6c-2d) консьюмит и
/// финализирует ledger по `propose_key`; `ledger_action_id` адресует [`AgentEvent::ExecResult`] (6c-2g).
/// `undo_eligible` (6c-2h) — **host-authority над обратимостью**: вычислен из СОХРАНЁННОГО действия на
/// execute (= это GitOp), НЕ из claim контейнера. report персистит `undo_ref` ТОЛЬКО при `undo_eligible`
/// (контейнер не сделает shell/process «обратимым», подсунув undo_ref для не-GitOp).
struct InFlightExec {
    propose_key: String,
    ledger_action_id: i64,
    undo_eligible: bool,
}

/// Каноническая repr exec-действия для fingerprint токена (US-разделитель `\u{1f}`). vault-таргеты сюда не
/// приходят (decide — exec-only); их арм пуст (fingerprint не используется для vault).
fn exec_fingerprint(action: &Action) -> String {
    match &action.target {
        ActionTarget::ShellRun { argv, cwd_rel } => format!(
            "shell\u{1f}{}\u{1f}{}",
            argv.join("\u{1f}"),
            cwd_rel.as_deref().unwrap_or("")
        ),
        ActionTarget::ProcessSpawn {
            program,
            args,
            cwd_rel,
        } => format!(
            "proc\u{1f}{program}\u{1f}{}\u{1f}{}",
            args.join("\u{1f}"),
            cwd_rel.as_deref().unwrap_or("")
        ),
        ActionTarget::GitOp { op, args } => format!("git\u{1f}{op}\u{1f}{}", args.join("\u{1f}")),
        // vault/skill-таргеты сюда не приходят (decide — exec-only); арм пуст (fingerprint не для них).
        ActionTarget::NoteCreate { .. }
        | ActionTarget::NoteEdit { .. }
        | ActionTarget::Frontmatter { .. }
        | ActionTarget::SkillSave { .. } => String::new(),
    }
}

/// Зарезервированные env-ключи: ВСЕГДА из фиксированного безопасного набора, skill-passthrough их НЕ
/// переопределяет (fail-closed — скилл не подменит PATH на writable-каталог с подброшенным бинарём).
const RESERVED_ENV_KEYS: [&str; 3] = ["PATH", "LANG", "HOME"];

/// Валиден ли `s` как git-ref для undo (§5.5, 6c-2h): непустой, ≤64 hex-символов (SHA-1=40/SHA-256=64).
/// **HOST-AUTHORITY контроль**: ОБЯЗАН проверяться на ДОВЕРЕННОЙ стороне (host `report`) ПЕРЕД персистом в
/// ledger — НЕ полагаться на in-container probe (`capture_pre_op_gitref` бежит на НЕдоверенной стороне:
/// скомпрометированный контейнер мог бы прислать `report{undo_ref:"HEAD; rm -rf ~"}` мимо probe). Отвергает
/// инъекц-/мусор-строки → `report` персистит `undo=None` (необратимо, fail-closed). Тот же предикат
/// переиспользует probe (единый источник правила). Pure, проверяемо на любом хосте.
pub(crate) fn is_git_sha(s: &str) -> bool {
    !s.is_empty() && s.len() <= 64 && s.chars().all(|c| c.is_ascii_hexdigit())
}

/// Строит env exec-команды (§5.4) — fail-CLOSED: из ПУСТОГО + фиксированный безопасный набор
/// (`PATH`/`LANG` + `HOME=scratch_home`) + явный per-skill `skill_passthrough` (КРОМЕ зарезервированных
/// ключей — их фикс-значения неприкосновенны). **НИКОГДА не читает `std::env` хоста** (структурно
/// fail-closed, не best-effort-скруб host-env). Denylist ЗАПРЕЩЁН by-design (fail-OPEN: секрет в
/// креативно-названной переменной / в `HOME` утёк бы). `skill_passthrough` — типизированный шов, дефолт
/// пуст (источник `SKILL.md::env_passthrough` ещё не в `GatedToolCtx` — отдельный skill-integration срез).
///
/// **Hard-gate для skill-integration (НЕ дрейфить):** когда `env_passthrough` оживёт, опасные dynamic-
/// linker / shell-inject имена (`LD_PRELOAD`/`LD_LIBRARY_PATH`/`LD_AUDIT`/`IFS`/`BASH_ENV`/…) должны
/// вето'аться НЕ fail-open-denylist'ом ЗДЕСЬ, а на уровне skill trust-gate/capability (вет источника
/// passthrough), консистентно с allow-list-доктриной. Сейчас дыры нет — passthrough пуст by-construction.
pub(crate) fn build_exec_env(
    scratch_home: &str,
    skill_passthrough: &[(String, String)],
) -> Vec<(String, String)> {
    let mut env = vec![
        (
            "PATH".to_string(),
            "/usr/local/bin:/usr/bin:/bin".to_string(),
        ),
        ("LANG".to_string(), "C.UTF-8".to_string()),
        ("HOME".to_string(), scratch_home.to_string()),
    ];
    for (k, v) in skill_passthrough {
        // Зарезервированный ключ из passthrough игнорируется (фикс-значение приоритетно, fail-closed).
        if RESERVED_ENV_KEYS.contains(&k.as_str()) {
            continue;
        }
        env.push((k.clone(), v.clone()));
    }
    env
}

/// Строит сигнал «исполни» ([`WireExecGo`]) host-side из СОХРАНЁННОГО [`Action`] (argv — host-authority:
/// контейнер argv не переподаёт → TOCTOU-замок approve-`ls`-run-`rm`). Exhaustive по 3 exec-таргетам;
/// vault-таргеты сюда не приходят (decide exec-only) → fail-closed пустой argv (исполнитель даст
/// launch_failure). env — allow-list ([`build_exec_env`]); cwd — scratch-tmpfs (`cwd_rel` действия;
/// `VaultRo` отложен — решит live 6c-3 по нужде `git.op`); таймаут/кэп — дефолты [`super`]. Вызывает redeem
/// (6c-2c) — здесь плита под него + Tier-1-тесты.
///
/// **cwd-конфайнмент (hard-gate, НЕ дрейфить):** `cwd_rel` кладётся в `ScratchTmpfs{rel}` БЕЗ валидации
/// ЗДЕСЬ намеренно — единственный чокпоинт конфайнмента — [`super::exec_child::resolve_cwd`] (6c-2a),
/// который применяет `classify::path_confinement` (отвергает `..`/abs/backslash/dot → команда НЕ
/// запускается) ВНУТРИ контейнера на exec. 6c-2c ОБЯЗАН гонять cwd ИМЕННО через `resolve_cwd`, не
/// `scratch_base.join(rel)` напрямую (иначе `cwd_rel="../etc"` сбежал бы из tmpfs внутри контейнера).
pub(crate) fn build_exec_go(action: &Action, skill_passthrough: &[(String, String)]) -> WireExecGo {
    let (argv, cwd_rel) = match &action.target {
        ActionTarget::ShellRun { argv, cwd_rel } => (argv.clone(), cwd_rel.clone()),
        ActionTarget::ProcessSpawn {
            program,
            args,
            cwd_rel,
        } => {
            let mut a = Vec::with_capacity(1 + args.len());
            a.push(program.clone());
            a.extend(args.iter().cloned());
            (a, cwd_rel.clone())
        }
        ActionTarget::GitOp { op, args } => {
            let mut a = Vec::with_capacity(2 + args.len());
            a.push("git".to_string());
            a.push(op.clone());
            a.extend(args.iter().cloned());
            (a, None)
        }
        // vault/skill-таргеты сюда не приходят (exec-only) → пустой argv (исполнитель даст launch_failure).
        ActionTarget::NoteCreate { .. }
        | ActionTarget::NoteEdit { .. }
        | ActionTarget::Frontmatter { .. }
        | ActionTarget::SkillSave { .. } => (Vec::new(), None),
    };
    WireExecGo {
        argv,
        cwd: ExecCwd::ScratchTmpfs {
            rel: cwd_rel.unwrap_or_default(),
        },
        env: build_exec_env(super::CONTAINER_SCRATCH, skill_passthrough),
        timeout_ms: super::DEFAULT_EXEC_TIMEOUT_MS,
        output_cap_bytes: super::DEFAULT_EXEC_OUTPUT_CAP,
    }
}

/// Потолок неисполненных одобренных exec (anti-рост, defense-in-depth). Без redeem (6c-2) store только
/// растёт; Approve требует решения [`DecisionSource`] (PolicyDefault DENY / человек) → рост и так ограничен
/// числом одобрений за прогон, но кэп — fail-closed предел на runaway-коннектор. 6c-2 redeem КОНСЬЮМИТ токен
/// (одноразовость) → штатно store не приближается к кэпу. Щедрый (исполнение в 6c-2 следует за decide сразу).
const MAX_PENDING_EXEC: usize = 64;

/// РЕАЛЬНЫЙ host-side [`ExecBackend`] (SANDBOX-6c): держит per-run [`GatedToolCtx`] (ТОТ ЖЕ гейт-контекст,
/// что note-инструменты — единый policy-путь) + token-store. `decide()` → [`dispatch_exec_decision`]; на
/// Approve минтит ОДНОРАЗОВЫЙ `exec_token`, привязанный к (run_id, ledger action_id, fingerprint действия),
/// и СОХРАНЯЕТ действие. redeem/finalize (фазы execute/report) — 6c-2.
pub struct DispatchExecBackend {
    ctx: crate::actuator::GatedToolCtx,
    /// Одобренные, но ещё не redeem'нутые exec (decide→pending). Консьюмится execute (one-shot).
    pending: std::sync::Mutex<std::collections::HashMap<String, PendingExec>>,
    /// redeem'нутые, исполняемые exec (execute→in_flight, ledger=EXECUTING). Консьюмит report (6c-2d).
    in_flight: std::sync::Mutex<std::collections::HashMap<String, InFlightExec>>,
}

impl DispatchExecBackend {
    pub fn new(ctx: crate::actuator::GatedToolCtx) -> Self {
        Self {
            ctx,
            pending: std::sync::Mutex::new(std::collections::HashMap::new()),
            in_flight: std::sync::Mutex::new(std::collections::HashMap::new()),
        }
    }

    /// Одноразовый НЕугадываемый токен = blake3(run_id|action_id|fingerprint|16-байт RANDOM-nonce).
    /// Random-nonce делает токен непрогнозируемым БЕЗ опоры на секретность run_id/action_id (контейнер их
    /// и так не знает). Анти-TOCTOU (approve-`ls`-run-`rm`) держится НЕ на токене, а на host-side store: 6c-2
    /// redeem ищет токен в `pending` и берёт СОХРАНЁННОЕ действие (контейнер argv не переподаёт).
    fn mint_token(run_id: i64, action_id: i64, action: &Action) -> String {
        let mut nonce = [0u8; 16];
        getrandom::getrandom(&mut nonce).expect("системный RNG недоступен");
        let nonce_hex: String = nonce.iter().map(|b| format!("{b:02x}")).collect();
        let material = format!(
            "{run_id}\u{1f}{action_id}\u{1f}{}\u{1f}{nonce_hex}",
            exec_fingerprint(action)
        );
        blake3::hash(material.as_bytes()).to_hex().to_string()
    }

    /// Число висящих одобренных exec (для тестов — без раскрытия PendingExec).
    pub fn pending_count(&self) -> usize {
        self.pending.lock().expect("pending mutex").len()
    }

    /// Число redeem'нутых (исполняемых) exec — для тестов (proxy «transition→EXECUTING прошёл»).
    #[cfg(test)]
    fn in_flight_count(&self) -> usize {
        self.in_flight.lock().expect("in_flight mutex").len()
    }

    /// Ledger-строка по propose_key (для тестов: проверка state/outcome финализации + приватности).
    #[cfg(test)]
    async fn ledger_row(&self, propose_key: &str) -> Option<crate::actuator::audit::ActionRow> {
        audit::lookup(&self.ctx.ledger.reader_handle(), propose_key)
            .await
            .ok()
            .flatten()
    }

    /// Тест-хелпер: propose_key единственной висящей записи (проверка, что 6c-2b его сохранил).
    #[cfg(test)]
    fn only_pending_propose_key(&self) -> Option<String> {
        let pending = self.pending.lock().expect("pending mutex");
        if pending.len() != 1 {
            return None;
        }
        pending.values().next().map(|p| p.propose_key.clone())
    }

    /// Тест-хелпер: набить store фиктивными записями (проверка soft-cap без N реальных одобрений).
    #[cfg(test)]
    fn force_fill_pending(&self, n: usize) {
        let mut pending = self.pending.lock().expect("pending mutex");
        for i in 0..n {
            pending.insert(
                format!("dummy-{i}"),
                PendingExec {
                    action: Action::shell_run(vec!["x".into()], None),
                    ledger_action_id: i as i64,
                    propose_key: format!("dummy-key-{i}"),
                },
            );
        }
    }
}

#[async_trait]
impl ExecBackend for DispatchExecBackend {
    async fn decide(&self, action: &Action) -> WireExecDecision {
        use crate::actuator::{dispatch_exec_decision, ExecDecision};
        // Soft-cap ПЕРЕД решением: при заполненном store отказываем ДО записи ledger-строки (чисто — нет
        // осиротевшей APPROVED-строки без токена). См. [`MAX_PENDING_EXEC`].
        if self.pending.lock().expect("pending mutex").len() >= MAX_PENDING_EXEC {
            return WireExecDecision::Rejected {
                summary: "слишком много неисполненных одобренных exec — отказано (fail-closed)"
                    .into(),
            };
        }
        let decision = dispatch_exec_decision(
            action,
            self.ctx.run_id,
            &self.ctx.policy,
            &self.ctx.decision_source,
            self.ctx.ledger.as_ref(),
            self.ctx.canon_root.as_path(),
            self.ctx.events.as_ref(),
        )
        .await;
        match decision {
            ExecDecision::Approved {
                ledger_action_id,
                propose_key,
            } => {
                let token = Self::mint_token(self.ctx.run_id, ledger_action_id, action);
                self.pending.lock().expect("pending mutex").insert(
                    token.clone(),
                    PendingExec {
                        action: action.clone(),
                        ledger_action_id,
                        propose_key,
                    },
                );
                WireExecDecision::Approved {
                    exec_token: token,
                    ledger_action_id,
                }
            }
            ExecDecision::Rejected(s) => WireExecDecision::Rejected { summary: s },
            ExecDecision::HardBlocked(r) => WireExecDecision::HardBlocked { reason: r },
        }
    }

    /// Фаза execute (6c-2c, redeem): КОНСЬЮМИТ одноразовый токен из `pending` → проводит ledger
    /// `approved→executing` (write-before-act exec) → строит host-authority [`WireExecGo`]. Порядок
    /// security-критичен:
    ///  0. **anti-runaway**: `in_flight` ограничен симметрично [`MAX_PENDING_EXEC`] (до consume);
    ///  1. **consume под локом** (`remove`) — одноразовость by-construction: повтор/гонка найдёт токен
    ///     отсутствующим → `invalid_params` (TOCTOU-замок: argv берётся из СОХРАНЁННОГО действия, не из wire);
    ///  2. **KILL-SWITCH LAST-MOMENT re-check** — ПОСЛЕ consume, НЕПОСРЕДСТВЕННО перед write-before-act
    ///     (зеркало `apply_now` «сужение TOCTOU»): `transition` — это DB-`await`, и пауза могла взвестись в
    ///     окне до записи EXECUTING. Под паузой НЕ пишем и ВОЗВРАЩАЕМ токен в `pending` (approval переживает
    ///     un-pause: «token stays»). Раньше проверка стояла ДО consume под локом → оставляла await-окно,
    ///     где флип паузы пропускал запуск (review MAJOR, зеркало `apply_now` orchestrate.rs);
    ///  3. ledger CAS `APPROVED→EXECUTING` (`transition` фенсит `state=approved AND outcome IS NULL`) —
    ///     не promoted ⇒ гонка/не-approved ⇒ ошибка (токен уже консьюмнут, fail-closed);
    ///  4. запоминаем в `in_flight` для report-финализации (6c-2d).
    async fn execute(&self, exec_token: &str) -> Result<WireExecGo, RpcError> {
        // Шаг 0: anti-runaway симметрично pending-кэпу (до consume — при переполнении токен не трогаем).
        if self.in_flight.lock().expect("in_flight mutex").len() >= MAX_PENDING_EXEC {
            return Err(RpcError::internal(
                "exec: слишком много исполняемых exec — отказано (fail-closed)",
            ));
        }
        // Шаг 1: consume под локом (std Mutex — без .await внутри). Берём владение PendingExec.
        // Pause-проверка НЕ здесь, а last-moment (шаг 2) — закрыть await-окно до записи EXECUTING.
        let pending = {
            let mut store = self.pending.lock().expect("pending mutex");
            match store.remove(exec_token) {
                Some(p) => p,
                // неизвестный или уже консьюмнутый токен (one-shot replay) — fail-closed.
                None => return Err(RpcError::invalid_params()),
            }
        };
        // Шаг 2: KILL-SWITCH LAST-MOMENT — под паузой НЕ пишем + ВОЗВРАЩАЕМ токен (un-pause retry).
        if self.ctx.policy.is_paused() {
            self.pending
                .lock()
                .expect("pending mutex")
                .insert(exec_token.to_string(), pending);
            return Err(RpcError::internal(
                "exec: агент на паузе (kill-switch, last-moment) — исполнение подавлено",
            ));
        }
        // Шаг 3: ledger approved→executing (CAS). Вне лока (await).
        let promoted = audit::transition(
            &self.ctx.ledger.writer_handle(),
            &pending.propose_key,
            STATE_APPROVED,
            STATE_EXECUTING,
        )
        .await
        .unwrap_or(false);
        if !promoted {
            // Строка не в approved (гонка/двойной redeem/уже терминирована). Токен уже консьюмнут.
            return Err(RpcError::internal(
                "exec: ledger approved→executing не применён (не в состоянии approved)",
            ));
        }
        // Шаг 4: запомнить для report (6c-2d/2h) — финализация по propose_key. undo_eligible вычислен из
        // СОХРАНЁННОГО действия (host-authority над обратимостью): ТОЛЬКО GitOp обратим (pre-op git-ref);
        // shell/process — нет (и classify их не Auto). Контейнер не переопределит это claim'ом undo_ref.
        let undo_eligible = matches!(pending.action.target, ActionTarget::GitOp { .. });
        self.in_flight.lock().expect("in_flight mutex").insert(
            exec_token.to_string(),
            InFlightExec {
                propose_key: pending.propose_key,
                ledger_action_id: pending.ledger_action_id,
                undo_eligible,
            },
        );
        // argv/env/cwd host-authority из СОХРАНЁННОГО действия (контейнер их не переподаёт).
        Ok(build_exec_go(&pending.action, &[]))
    }

    /// Фаза report (6c-2d): КОНСЬЮМИТ `in_flight[token]` (one-shot финализация) → ledger
    /// `EXECUTING→EXECUTED|FAILED` (`exit_code==0` ⇒ EXECUTED). **Приватность**: ledger outcome —
    /// СТРУКТУРНОЕ резюме (exit + байт-счётчики хвостов), сырой stdout/stderr НЕ персистится (зеркало
    /// diff_summary). `undo_ref` (6c-2h GitOp pre-op-ref) персистится как [`UndoCols`]`{kind:exec_gitref}`
    /// ТОЛЬКО при `in_flight.undo_eligible` (host-authority: СОХРАНЁННОЕ действие — GitOp) — контейнер не
    /// сделает shell/process «обратимым» claim'ом. finish — CAS (outcome IS NULL): replay/гонка → false →
    /// ошибка. Нет in_flight (нет execute / двойной report) → invalid_params.
    async fn report(
        &self,
        exec_token: &str,
        exit_code: i32,
        stdout_tail: &str,
        stderr_tail: &str,
        undo_ref: Option<&str>,
    ) -> Result<WireExecResult, RpcError> {
        // Консьюм in_flight (one-shot): отсутствует ⇒ нет execute / повторный report ⇒ fail-closed.
        let in_flight = match self
            .in_flight
            .lock()
            .expect("in_flight mutex")
            .remove(exec_token)
        {
            Some(f) => f,
            None => return Err(RpcError::invalid_params()),
        };
        let state = if exit_code == 0 {
            STATE_EXECUTED
        } else {
            STATE_FAILED
        };
        // ПРИВАТНОСТЬ: только структурное резюме (exit + длины), НЕ сырой вывод (он — в ExecResult-событие
        // 6c-2g для транзитного UI, не в долговечный ledger).
        let outcome = format!(
            "exec exit={exit_code} (stdout {}B, stderr {}B)",
            stdout_tail.len(),
            stderr_tail.len()
        );
        // 6c-2h: undo_ref→UndoCols{exec_gitref} ТОЛЬКО для host-классифицированного GitOp (undo_eligible) И
        // валидного git-sha. shell/process (undo_eligible=false) ⇒ None (необратимы). **HOST-AUTHORITY над
        // СОДЕРЖИМЫМ ref** (review MAJOR): host НЕ доверяет in-container probe — РЕ-валидирует ref сам
        // ([`is_git_sha`]) на доверенной стороне ПЕРЕД персистом; мусор/инъекц-строка («HEAD; rm -rf ~») ⇒
        // None (необратимо, fail-closed). Реальный `git reset` по этому ref — 6c-3 (песочница под апрувом).
        let undo = match (in_flight.undo_eligible, undo_ref) {
            (true, Some(r)) if is_git_sha(r) => Some(UndoCols {
                kind: UNDO_EXEC_GITREF.to_string(),
                reference: r.to_string(),
                // exec-GitOp: fs-домена нет (откат — git reset драйвером, не restore под vault/skills_root).
                // NULL ⇒ читатель падёт на tool_name-fallback (git_op → Vault; root ExecGitRef не консультирует).
                domain: None,
            }),
            _ => None,
        };
        let finalized = audit::finish(
            &self.ctx.ledger.writer_handle(),
            &in_flight.propose_key,
            state,
            &outcome,
            undo,
        )
        .await
        .unwrap_or(false);
        if !finalized {
            // Строка уже терминальна (гонка/двойная финализация) — fail-closed.
            return Err(RpcError::internal(
                "exec: ledger финализация не применена (строка уже терминальна)",
            ));
        }
        // ExecResult (6c-2g): UI/лог видят исход. СОДЕРЖИМОЕ-СВОБОДЕН by-design — exit_code + finalized,
        // НЕ сырой stdout/stderr (приватность §5.6; вывод видит лишь модель через fenced tool-result).
        self.ctx
            .events
            .emit(crate::agent::event::AgentEvent::ExecResult {
                run_id: self.ctx.run_id,
                action_id: in_flight.ledger_action_id,
                exit_code,
                finalized: true,
            });
        Ok(WireExecResult {
            exit_code,
            finalized: true,
        })
    }
}

#[cfg(test)]
mod tests;
