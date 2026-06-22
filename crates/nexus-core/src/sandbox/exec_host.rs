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
//! **6c-1 (этот срез):** wire-DTO + `ExecBackend::decide` + `HostExecServer` (decide); execute/report —
//! зарезервированы (6c-2 `redeem`/`finalize` + контейнерный исполнитель). НИ ОДНОГО исполнения здесь.
//!
//! **6c-2 ОБЯЗАН (review hard-gates, флагнуто здесь чтобы инвариант не дрейфанул):**
//!  1. `redeem` КОНСЬЮМИТ токен из `pending` (одноразовость + прунинг роста store; см. [`MAX_PENDING_EXEC`]);
//!  2. `WireExecGo.env` строится ТОЛЬКО из env-allowlist (спека §5.4), НЕ из host-env — НИ ОДНОГО секрета;
//!  3. `serve_exec` оборачивает accept-путь в `peer_authorized` (`SO_PEERCRED`), как serve_act/egress/event —
//!     иначе любой локальный процесс гонял бы decide/redeem.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::actuator::{Action, ActionTarget};
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
}

/// Host-обработчик `host/exec`. 6c-1: маршрутизирует `decide`; `execute`/`report` зарезервированы (6c-2)
/// → `invalid_params` (контейнер 6c-1-образа их не шлёт; fail-closed, если кто-то пошлёт раньше времени).
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
            // 6c-2: исполнение ВНУТРИ песочницы (redeem токена → WireExecGo) + финализация ledger.
            WireExecPhase::Execute | WireExecPhase::Report => Err(RpcError::invalid_params()),
        }
    }
}

/// Запомненное одобренное exec-действие. Контейнер на `execute` (6c-2c) предъявит ТОЛЬКО `exec_token`;
/// host строит `WireExecGo` argv из ЭТОГО сохранённого действия (контейнер argv не переподаёт → TOCTOU-
/// замок approve-`ls`-run-`rm`). `propose_key` — idempotency-ключ ledger-строки (redeem/finalize фенсят
/// `approved→executing→executed|failed` по нему). Поля читает redeem/finalize (6c-2c/2d).
#[allow(dead_code)] // поля читаются в 6c-2c/2d (redeem/finalize); 6c-2b только минтит+хранит
struct PendingExec {
    action: Action,
    ledger_action_id: i64,
    propose_key: String,
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
        ActionTarget::NoteCreate { .. }
        | ActionTarget::NoteEdit { .. }
        | ActionTarget::Frontmatter { .. } => String::new(),
    }
}

/// Зарезервированные env-ключи: ВСЕГДА из фиксированного безопасного набора, skill-passthrough их НЕ
/// переопределяет (fail-closed — скилл не подменит PATH на writable-каталог с подброшенным бинарём).
const RESERVED_ENV_KEYS: [&str; 3] = ["PATH", "LANG", "HOME"];

/// Строит env exec-команды (§5.4) — fail-CLOSED: из ПУСТОГО + фиксированный безопасный набор
/// (`PATH`/`LANG` + `HOME=scratch_home`) + явный per-skill `skill_passthrough` (КРОМЕ зарезервированных
/// ключей — их фикс-значения неприкосновенны). **НИКОГДА не читает `std::env` хоста** (структурно
/// fail-closed, не best-effort-скруб host-env). Denylist ЗАПРЕЩЁН by-design (fail-OPEN: секрет в
/// креативно-названной переменной / в `HOME` утёк бы). `skill_passthrough` — типизированный шов, дефолт
/// пуст (источник `SKILL.md::env_passthrough` ещё не в `GatedToolCtx` — отдельный skill-integration срез).
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
#[allow(dead_code)] // зовётся redeem-фазой (6c-2c); 6c-2b строит+тестирует
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
        ActionTarget::NoteCreate { .. }
        | ActionTarget::NoteEdit { .. }
        | ActionTarget::Frontmatter { .. } => (Vec::new(), None),
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
    pending: std::sync::Mutex<std::collections::HashMap<String, PendingExec>>,
}

impl DispatchExecBackend {
    pub fn new(ctx: crate::actuator::GatedToolCtx) -> Self {
        Self {
            ctx,
            pending: std::sync::Mutex::new(std::collections::HashMap::new()),
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
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exec_actions() -> Vec<Action> {
        vec![
            Action::shell_run(vec!["ls".into(), "-la".into()], Some("Notes".into())),
            Action::process_spawn("git", vec!["status".into()], None),
            Action::git_op("log", vec!["--oneline".into()]),
        ]
    }

    #[test]
    fn wire_exec_action_roundtrip_all_exec_targets() {
        for a in exec_actions() {
            let wire = WireExecAction::try_from(&a).unwrap();
            let json = serde_json::to_string(&wire).unwrap();
            let back: WireExecAction = serde_json::from_str(&json).unwrap();
            let a2: Action = back.try_into().unwrap();
            assert_eq!(a, a2, "round-trip Action↔WireExecAction↔JSON: {a:?}");
        }
    }

    #[test]
    fn vault_target_not_representable_on_host_exec() {
        for a in [
            Action::note_create("A.md", "b"),
            Action::note_edit("B.md", "c"),
            Action::frontmatter("C.md", "k", "v"),
        ] {
            assert!(
                WireExecAction::try_from(&a).is_err(),
                "vault не на host/exec: {a:?}"
            );
        }
    }

    #[test]
    fn wire_exec_request_rejects_unknown_field() {
        let json = r#"{"phase":"decide","action":{"kind":"git_op","op":"status"},"bogus":1}"#;
        assert!(serde_json::from_str::<WireExecRequest>(json).is_err());
    }

    /// Мок-бэкенд: возвращает заданное решение (без classify/ledger).
    struct MockExec(WireExecDecision);
    #[async_trait]
    impl ExecBackend for MockExec {
        async fn decide(&self, _action: &Action) -> WireExecDecision {
            self.0.clone()
        }
    }

    #[tokio::test]
    async fn host_exec_server_decide_maps_approved() {
        let srv = HostExecServer::new(MockExec(WireExecDecision::Approved {
            exec_token: "tok-1".into(),
            ledger_action_id: 7,
        }));
        let req = WireExecRequest {
            phase: WireExecPhase::Decide,
            action: Some(WireExecAction::try_from(&Action::git_op("status", vec![])).unwrap()),
            exec_token: None,
            exit_code: None,
            stdout_tail: None,
            stderr_tail: None,
            undo_ref: None,
        };
        let out = srv
            .handle(HOST_EXEC, serde_json::to_value(req).unwrap())
            .await
            .unwrap();
        let dec: WireExecDecision = serde_json::from_value(out).unwrap();
        assert_eq!(
            dec,
            WireExecDecision::Approved {
                exec_token: "tok-1".into(),
                ledger_action_id: 7
            }
        );
    }

    #[tokio::test]
    async fn host_exec_server_execute_phase_reserved_6c2() {
        let srv = HostExecServer::new(MockExec(WireExecDecision::Rejected {
            summary: "x".into(),
        }));
        let req = WireExecRequest {
            phase: WireExecPhase::Execute,
            action: None,
            exec_token: Some("tok".into()),
            exit_code: None,
            stdout_tail: None,
            stderr_tail: None,
            undo_ref: None,
        };
        // 6c-1: execute зарезервирован → invalid_params (6c-2 включит исполнение).
        assert!(srv
            .handle(HOST_EXEC, serde_json::to_value(req).unwrap())
            .await
            .is_err());
    }

    #[tokio::test]
    async fn host_exec_unknown_method_not_found() {
        let srv = HostExecServer::new(MockExec(WireExecDecision::Rejected {
            summary: "x".into(),
        }));
        assert!(srv.handle("host/act", Value::Null).await.is_err());
    }

    /// fail-closed: decide-запрос с execute/report-полем (exec_token) отвергается (кросс-фазовый mix).
    #[tokio::test]
    async fn host_exec_decide_rejects_cross_phase_fields() {
        let srv = HostExecServer::new(MockExec(WireExecDecision::Rejected {
            summary: "unreached".into(),
        }));
        let json = serde_json::json!({
            "phase": "decide",
            "action": {"kind": "git_op", "op": "status"},
            "exec_token": "smuggled",
        });
        assert!(
            srv.handle(HOST_EXEC, json).await.is_err(),
            "decide с exec_token → invalid_params"
        );
    }

    // ── DispatchExecBackend end-to-end (Tier-1: настоящий vault+БД+ledger, classify_exec+decision) ──
    use crate::actuator::{
        AuditSink, ChannelDecision, DecisionSource, DispatchPolicy, EventSink, GatedToolCtx,
        ItemDecision, PolicyDefault, TracingEventSink, OVERWRITE_THRESHOLD,
    };
    use crate::db::Database;
    use std::sync::Arc;
    use tempfile::TempDir;

    /// Реальный GatedToolCtx с exec-флагами (shell_enable/sandbox_available) + источником решений.
    async fn exec_gate(
        shell_enable: bool,
        sandbox_available: bool,
        decision: Arc<dyn DecisionSource>,
    ) -> (TempDir, DispatchExecBackend) {
        let dir = TempDir::new().unwrap();
        let canon_root = dir.path().canonicalize().unwrap();
        let db = Database::open(canon_root.join(".nexus/nexus.db"))
            .await
            .unwrap();
        let ledger = AuditSink::new(db.writer().clone(), db.reader().clone());
        std::mem::forget(db);
        let policy = DispatchPolicy::new(Some("auto"), OVERWRITE_THRESHOLD, 16)
            .with_exec_flags(shell_enable, sandbox_available);
        let events: Arc<dyn EventSink> = Arc::new(TracingEventSink::new());
        let ctx = GatedToolCtx::new(canon_root, ledger, 1, policy, decision, events);
        (dir, DispatchExecBackend::new(ctx))
    }

    /// shell_enable=false → HardBlocked(ShellDisabled), токен НЕ выдан (pending пуст).
    #[tokio::test]
    async fn dispatch_exec_shell_disabled_hardblocked_no_token() {
        let (_d, backend) = exec_gate(false, true, Arc::new(PolicyDefault)).await;
        let dec = backend.decide(&Action::git_op("status", vec![])).await;
        assert!(
            matches!(dec, WireExecDecision::HardBlocked { .. }),
            "dec={dec:?}"
        );
        assert_eq!(backend.pending_count(), 0, "HardBlocked не минтит токен");
    }

    /// shell_enable+sandbox, но PolicyDefault (DENY headless) → Rejected, токен НЕ выдан.
    #[tokio::test]
    async fn dispatch_exec_policy_default_rejected_no_token() {
        let (_d, backend) = exec_gate(true, true, Arc::new(PolicyDefault)).await;
        let dec = backend
            .decide(&Action::shell_run(vec!["ls".into()], None))
            .await;
        assert!(
            matches!(dec, WireExecDecision::Rejected { .. }),
            "dec={dec:?}"
        );
        assert_eq!(backend.pending_count(), 0, "Rejected не минтит токен");
    }

    /// shell_enable+sandbox + Approve (ChannelDecision) → Approved + одноразовый токен сохранён.
    #[tokio::test]
    async fn dispatch_exec_approved_mints_token() {
        // action_id первой proposed-строки в пустой БД = 1; засеваем Approve по id=1.
        let (chan, tx) = ChannelDecision::new(1);
        tx.send(crate::actuator::BatchDecision::from_pairs([(
            1,
            ItemDecision::Approve,
        )]))
        .await
        .unwrap();
        let (_d, backend) = exec_gate(true, true, Arc::new(chan)).await;
        let dec = backend
            .decide(&Action::shell_run(vec!["echo".into(), "hi".into()], None))
            .await;
        match dec {
            WireExecDecision::Approved {
                exec_token,
                ledger_action_id,
            } => {
                assert!(!exec_token.is_empty(), "токен непуст");
                assert_eq!(ledger_action_id, 1);
                assert_eq!(
                    backend.pending_count(),
                    1,
                    "одобренный exec сохранён под токеном"
                );
                // 6c-2b: PendingExec несёт непустой propose_key (ledger-фенс redeem/finalize).
                assert!(
                    backend
                        .only_pending_propose_key()
                        .is_some_and(|k| !k.is_empty()),
                    "propose_key сохранён непустым"
                );
            }
            other => panic!("ожидался Approved, получено {other:?}"),
        }
    }

    // ── 6c-2b: build_exec_env (allow-list §5.4) ──────────────────────────────────────────────────
    #[test]
    fn build_exec_env_is_allowlist_only() {
        std::env::set_var("NEXUS_FAKE_SECRET", "leaked");
        let env = build_exec_env("/tmp", &[]);
        std::env::remove_var("NEXUS_FAKE_SECRET");
        let keys: Vec<&str> = env.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, vec!["PATH", "LANG", "HOME"], "только фикс-набор");
        assert!(
            !env.iter().any(|(k, _)| k == "NEXUS_FAKE_SECRET"),
            "host-секрет НЕ просочился (build_exec_env не читает std::env)"
        );
    }

    #[test]
    fn build_exec_env_home_is_scratch() {
        let env = build_exec_env("/tmp", &[]);
        assert_eq!(
            env.iter()
                .find(|(k, _)| k == "HOME")
                .map(|(_, v)| v.as_str()),
            Some("/tmp"),
            "HOME = scratch (не host HOME)"
        );
    }

    #[test]
    fn build_exec_env_includes_declared_passthrough() {
        let env = build_exec_env("/tmp", &[("FOO".into(), "bar".into())]);
        assert!(
            env.iter().any(|(k, v)| k == "FOO" && v == "bar"),
            "объявленный passthrough присутствует"
        );
    }

    /// fail-closed: skill-passthrough НЕ переопределяет зарезервированные PATH/HOME/LANG.
    #[test]
    fn build_exec_env_passthrough_cannot_override_reserved() {
        let env = build_exec_env(
            "/tmp",
            &[
                ("PATH".into(), "/evil".into()),
                ("HOME".into(), "/evil".into()),
            ],
        );
        let path = env
            .iter()
            .find(|(k, _)| k == "PATH")
            .map(|(_, v)| v.as_str());
        let home = env
            .iter()
            .find(|(k, _)| k == "HOME")
            .map(|(_, v)| v.as_str());
        assert_eq!(
            path,
            Some("/usr/local/bin:/usr/bin:/bin"),
            "PATH из фикс-набора, не из skill"
        );
        assert_eq!(home, Some("/tmp"), "HOME из scratch, не из skill");
        // и НЕ продублирован
        assert_eq!(
            env.iter().filter(|(k, _)| k == "PATH").count(),
            1,
            "PATH не задублирован"
        );
    }

    // ── 6c-2b: build_exec_go (argv host-authority + дефолты) ──────────────────────────────────────
    #[test]
    fn build_exec_go_argv_from_action() {
        let g = build_exec_go(&Action::git_op("status", vec!["--short".into()]), &[]);
        assert_eq!(g.argv, vec!["git", "status", "--short"]);
        let g = build_exec_go(
            &Action::shell_run(vec!["ls".into(), "-la".into()], None),
            &[],
        );
        assert_eq!(g.argv, vec!["ls", "-la"]);
        let g = build_exec_go(&Action::process_spawn("rg", vec!["foo".into()], None), &[]);
        assert_eq!(g.argv, vec!["rg", "foo"]);
    }

    #[test]
    fn build_exec_go_defaults_scratch_cwd_and_caps() {
        let g = build_exec_go(
            &Action::shell_run(vec!["ls".into()], Some("sub".into())),
            &[],
        );
        assert_eq!(g.cwd, ExecCwd::ScratchTmpfs { rel: "sub".into() });
        assert_eq!(g.timeout_ms, super::super::DEFAULT_EXEC_TIMEOUT_MS);
        assert_eq!(g.output_cap_bytes, super::super::DEFAULT_EXEC_OUTPUT_CAP);
        // env = allow-list
        let keys: Vec<&str> = g.env.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, vec!["PATH", "LANG", "HOME"]);
    }

    #[test]
    fn build_exec_go_no_cwd_defaults_empty_scratch() {
        let g = build_exec_go(&Action::git_op("log", vec![]), &[]);
        assert_eq!(g.cwd, ExecCwd::ScratchTmpfs { rel: String::new() });
    }

    /// Soft-cap: при заполненном store decide отказывает ДО записи ledger (новый токен не добавлен).
    #[tokio::test]
    async fn dispatch_exec_pending_soft_cap_rejects() {
        // PolicyDefault — но до источника решений не дойдём: cap-чек срабатывает раньше.
        let (_d, backend) = exec_gate(true, true, Arc::new(PolicyDefault)).await;
        backend.force_fill_pending(MAX_PENDING_EXEC);
        let dec = backend
            .decide(&Action::shell_run(vec!["ls".into()], None))
            .await;
        assert!(
            matches!(dec, WireExecDecision::Rejected { .. }),
            "at-cap → Rejected, dec={dec:?}"
        );
        assert_eq!(
            backend.pending_count(),
            MAX_PENDING_EXEC,
            "кэп не превышен — новый exec не добавлен"
        );
    }
}
