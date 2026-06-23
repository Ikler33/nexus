//! `DelegateTool` (SUB-3b-2a) — инструмент `delegate.run`: СИНХРОННЫЙ (для родителя) fan-out субагентов.
//! Родитель зовёт `delegate.run` с батчем задач → инструмент гоняет [`spawn_subagent`] по каждой
//! КОНКУРЕНТНО ([`tokio::task::JoinSet`]; `max_fanout` = кап И размера батча, И конкурентности), ДОЖИДАЕТСЯ всех и
//! возвращает АГРЕГИРОВАННЫЙ JSON `[{index,status,summary}]` — родитель видит ТОЛЬКО эти саммари (контракт
//! изоляции). Один упавший ребёнок НЕ валит батч (его запись — `status:"failed"`). Регистрация инструмента
//! (под `ai.delegation.enabled`) + проводка хендлов — SUB-3b-2b.
//!
//! Анти-runaway СТРУКТУРНО: батч > `max_fanout` → `BadArgs` (модель уменьшит); каждый спавн fail-closed
//! списывает общий [`super::DelegationBudget`] внутри [`spawn_subagent`] (исчерпан → `failed`-запись, без
//! спавна); рекурсия исключена (дети не получают `delegate.run` — blocklist SUB-1).

use async_trait::async_trait;
use serde::Deserialize;

use crate::agent::event::SubagentState;
use crate::agent::tool::{Tool, ToolError, ToolSpec};

use super::spawn::{spawn_subagent, SubagentContext, SubagentResult};

/// Имя инструмента (совпадает с [`super::DELEGATE_RUN_TOOL`] — он же в блок-листе реестра ребёнка).
pub const DELEGATE_TOOL: &str = super::DELEGATE_RUN_TOOL;

/// Жёсткий потолок числа задач в ОДНОМ вызове (defense-in-depth поверх конфиг-`max_fanout`): даже если
/// `max_fanout` задан безумно большим, один `delegate.run` не породит лавину. Конкретный кап = min(этого и
/// конфигурного).
const HARD_FANOUT_CEILING: usize = 16;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DelegateArgs {
    /// Батч подзадач (1..=max_fanout). Пустой → BadArgs.
    tasks: Vec<DelegateTask>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DelegateTask {
    /// Что должен сделать субагент (становится его фокус-задачей через `build_child_task`).
    goal: String,
    /// Опц. доп. контекст (без истории родителя).
    #[serde(default)]
    context: Option<String>,
    /// Опц. СУЖАЮЩИЙ запрос инструментов (⊆ родителя; имя сверх — отбрасывается, не добавляется).
    #[serde(default)]
    tools: Option<Vec<String>>,
}

/// Инструмент делегирования. Держит общий [`SubagentContext`] (клонируется в каждую конкурентную задачу)
/// и эффективный `max_fanout`.
pub struct DelegateTool {
    ctx: SubagentContext,
    max_fanout: usize,
}

impl DelegateTool {
    /// `max_fanout` нормализуется к `1..=HARD_FANOUT_CEILING` (0 → 1; сверх потолка → потолок).
    pub fn new(ctx: SubagentContext, max_fanout: usize) -> Self {
        Self {
            ctx,
            max_fanout: max_fanout.clamp(1, HARD_FANOUT_CEILING),
        }
    }
}

fn state_str(s: SubagentState) -> &'static str {
    match s {
        SubagentState::Done => "done",
        SubagentState::Failed => "failed",
        SubagentState::Paused => "paused",
        SubagentState::Spawned | SubagentState::Running => "running",
    }
}

#[async_trait]
impl Tool for DelegateTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: DELEGATE_TOOL.into(),
            description: format!(
                "Делегировать подзадачи СУБАГЕНТАМ (фокус-исполнители). Каждая задача — \
                 {{goal, context?, tools?}}; вернётся массив [{{index,status,summary}}] (видишь ТОЛЬКО \
                 краткие саммари детей). До {} задач за вызов. Субагент не может делегировать дальше.",
                self.max_fanout
            ),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "tasks": {
                        "type": "array",
                        "minItems": 1,
                        "items": {
                            "type": "object",
                            "properties": {
                                "goal": {"type": "string", "description": "Что сделать субагенту"},
                                "context": {"type": "string", "description": "Доп. контекст (опц.)"},
                                "tools": {
                                    "type": "array",
                                    "items": {"type": "string"},
                                    "description": "Сузить инструменты ребёнка (опц., ⊆ родителя)"
                                }
                            },
                            "required": ["goal"],
                            "additionalProperties": false
                        }
                    }
                },
                "required": ["tasks"],
                "additionalProperties": false
            }),
        }
    }

    async fn invoke(&self, args: &str) -> Result<String, ToolError> {
        let parsed: DelegateArgs =
            serde_json::from_str(args).map_err(|e| ToolError::BadArgs(e.to_string()))?;
        if parsed.tasks.is_empty() {
            return Err(ToolError::BadArgs("tasks пуст (нужна ≥1 задача)".into()));
        }
        if parsed.tasks.len() > self.max_fanout {
            return Err(ToolError::BadArgs(format!(
                "слишком много задач за вызов: {} > предел {} (раздели на меньшие батчи)",
                parsed.tasks.len(),
                self.max_fanout
            )));
        }

        let n = parsed.tasks.len();
        // Конкурентный fan-out. `max_fanout` ОДНОВРЕМЕННО кап размера батча (проверен выше) И кап
        // конкурентности (батч ≤ max_fanout → не более max_fanout детей разом; дефолт 3 — щадяще для
        // одного GPU). Общий `DelegationBudget` внутри `spawn_subagent` дополнительно ограничивает СУММУ
        // спавнов дерева.
        // ⚠ ABORT-ON-DROP (ревью SUB-3b-2a MAJOR #4): `JoinSet` при дропе фьючи-инструмента (отмена/таймаут
        // родителя) аборнет детей НА `.await` — возможны осиротевшие `running`-строки `agent_runs` +
        // несписанный budget. Закрывается на проводке (SUB-3b-2b): agentd на старте зовёт
        // `run_store::reconcile_orphan_child_runs` (стартап-sweep, как requeue_stale_running) ДО регистрации
        // `delegate.run`. Это ЖЁСТКОЕ предусловие активации (см. doc reconcile_orphan_child_runs).
        let mut set = tokio::task::JoinSet::new();
        for (i, t) in parsed.tasks.into_iter().enumerate() {
            let ctx = self.ctx.clone(); // дёшево: всё внутри Arc/Clone
            set.spawn(async move {
                let res =
                    spawn_subagent(&ctx, &t.goal, t.context.as_deref(), t.tools.as_deref()).await;
                (i, res)
            });
        }

        // Сбор ПО ИНДЕКСУ (порядок батча сохранён). Дренаж ВСЕХ задач: один упавший ребёнок НЕ валит батч.
        let mut results: Vec<Option<SubagentResult>> = (0..n).map(|_| None).collect();
        while let Some(joined) = set.join_next().await {
            match joined {
                Ok((i, res)) => {
                    if let Some(slot) = results.get_mut(i) {
                        *slot = Some(res);
                    }
                }
                // Паника задачи (баг нашего кода, не отказ ребёнка) — единственный путь «сломан наш код»:
                // логируем; слот остаётся None → синтетическая failed-запись ниже. Сиблинги не теряются.
                Err(e) => {
                    tracing::error!(error = %e, "subagent-задача delegate.run паниковала")
                }
            }
        }

        let arr: Vec<serde_json::Value> = results
            .into_iter()
            .enumerate()
            .map(|(i, r)| match r {
                Some(res) => serde_json::json!({
                    "index": i,
                    "status": state_str(res.state),
                    "summary": res.summary,
                }),
                None => serde_json::json!({
                    "index": i,
                    "status": "failed",
                    "summary": "(субагент: внутренняя ошибка задачи)",
                }),
            })
            .collect();
        Ok(serde_json::to_string(&arr).unwrap_or_else(|_| "[]".to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actuator::PolicyDefault;
    use crate::agent::session::AgentEventForwarder;
    use crate::agent::tool::ToolSpec as _ToolSpec;
    use crate::ai::tools::{ToolCapableProvider, ToolTurn};
    use crate::ai::{AiResult, ChatMessage};
    use crate::db::Database;
    use crate::net::RunCtx;
    use std::collections::BTreeSet;
    use std::sync::atomic::AtomicBool;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;
    use tempfile::TempDir;

    use super::super::budget::DelegationBudget;

    /// Провайдер с фиксированным ходом для КАЖДОГО прогона ребёнка (без очереди — fan-out параллелен).
    struct AlwaysProvider {
        turn: Box<dyn Fn() -> AiResult<ToolTurn> + Send + Sync>,
    }
    #[async_trait]
    impl ToolCapableProvider for AlwaysProvider {
        async fn stream_chat_tools(
            &self,
            _m: &[ChatMessage],
            _t: &[_ToolSpec],
            _on: &mut (dyn FnMut(String) + Send),
            _c: &Arc<AtomicBool>,
            _ctx: RunCtx,
        ) -> AiResult<ToolTurn> {
            (self.turn)()
        }
        fn model_id(&self) -> &str {
            "fake"
        }
    }

    #[derive(Default)]
    struct NullForwarder(Mutex<usize>);
    impl AgentEventForwarder for NullForwarder {
        fn forward(&self, _ev: &crate::agent::event::AgentEvent) {
            *self.0.lock().unwrap() += 1;
        }
    }

    async fn make_ctx(
        dir: &TempDir,
        db: &Database,
        provider: Arc<dyn ToolCapableProvider>,
    ) -> SubagentContext {
        SubagentContext {
            provider,
            skills: None,
            web: None,
            decision_source: Arc::new(PolicyDefault),
            writer: db.writer().clone(),
            reader: db.reader().clone(),
            paused: Arc::new(AtomicBool::new(false)),
            parent_cancel: Arc::new(AtomicBool::new(false)),
            forwarder: Arc::new(NullForwarder::default()),
            parent_run_id: 1,
            parent_tool_names: ["debug.echo".to_string()]
                .into_iter()
                .collect::<BTreeSet<_>>(),
            dispatcher: None,
            actuator_enabled: false,
            autonomy: None,
            overwrite_threshold: 100,
            blast_cap: 10,
            context_window: Some(4096),
            canon_root: dir.path().to_path_buf(),
            model: Some("fake".into()),
            budget: DelegationBudget::new(1, 8, 3, Duration::from_secs(3600)),
        }
    }

    async fn db() -> (TempDir, Database) {
        let dir = TempDir::new().unwrap();
        let db = Database::open(dir.path().join("t.db")).await.unwrap();
        (dir, db)
    }

    /// Два таска → конкурентный fan-out → агрегат [{index,status,summary}] из ДВУХ записей, обе done,
    /// порядок батча сохранён. Родитель видит только саммари.
    #[tokio::test]
    async fn delegate_two_tasks_aggregates_summaries() {
        let (dir, dbh) = db().await;
        let provider = Arc::new(AlwaysProvider {
            turn: Box::new(|| Ok(ToolTurn::Final("итог".into()))),
        });
        let ctx = make_ctx(&dir, &dbh, provider).await;
        let tool = DelegateTool::new(ctx, 3);
        let out = tool
            .invoke(r#"{"tasks":[{"goal":"a"},{"goal":"b"}]}"#)
            .await
            .expect("delegate ok");
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        let arr = v.as_array().unwrap();
        assert_eq!(arr.len(), 2, "две записи");
        assert_eq!(arr[0]["index"], 0);
        assert_eq!(arr[1]["index"], 1);
        assert_eq!(arr[0]["status"], "done");
        assert_eq!(arr[1]["status"], "done");
        assert_eq!(arr[0]["summary"], "итог");
        // Дерево: два ребёнка с parent_run_id=1.
        let kids: i64 = dbh
            .reader()
            .query(|c| {
                c.query_row(
                    "SELECT count(*) FROM agent_runs WHERE parent_run_id=1",
                    [],
                    |r| r.get(0),
                )
            })
            .await
            .unwrap();
        assert_eq!(kids, 2);
    }

    /// Батч сверх max_fanout → BadArgs (модель уменьшит), спавнов НЕ было.
    #[tokio::test]
    async fn delegate_fanout_over_cap_rejected() {
        let (dir, dbh) = db().await;
        let provider = Arc::new(AlwaysProvider {
            turn: Box::new(|| Ok(ToolTurn::Final("x".into()))),
        });
        let ctx = make_ctx(&dir, &dbh, provider).await;
        let tool = DelegateTool::new(ctx, 2);
        let err = tool
            .invoke(r#"{"tasks":[{"goal":"a"},{"goal":"b"},{"goal":"c"}]}"#)
            .await;
        assert!(
            matches!(err, Err(ToolError::BadArgs(_))),
            "3 > cap 2 → BadArgs"
        );
        let kids: i64 = dbh
            .reader()
            .query(|c| c.query_row("SELECT count(*) FROM agent_runs", [], |r| r.get(0)))
            .await
            .unwrap();
        assert_eq!(kids, 0, "ни одного спавна при отказе");
    }

    /// Пустой батч / лишнее поле → BadArgs (fail-closed разбор).
    #[tokio::test]
    async fn delegate_empty_and_bad_args_rejected() {
        let (dir, dbh) = db().await;
        let provider = Arc::new(AlwaysProvider {
            turn: Box::new(|| Ok(ToolTurn::Final("x".into()))),
        });
        let ctx = make_ctx(&dir, &dbh, provider).await;
        let tool = DelegateTool::new(ctx, 3);
        assert!(matches!(
            tool.invoke(r#"{"tasks":[]}"#).await,
            Err(ToolError::BadArgs(_))
        ));
        assert!(matches!(
            tool.invoke(r#"{"tasks":[{"goal":"a","oops":1}]}"#).await,
            Err(ToolError::BadArgs(_))
        ));
        assert!(matches!(
            tool.invoke(r#"{"nope":1}"#).await,
            Err(ToolError::BadArgs(_))
        ));
    }

    /// Один упавший ребёнок НЕ валит батч: оба таска отрабатывают, агрегат содержит обе записи
    /// (здесь оба failed — провайдер возвращает ошибку; ключ — батч ЗАВЕРШИЛСЯ, не оборвался).
    #[tokio::test]
    async fn delegate_child_failure_does_not_abort_batch() {
        let (dir, dbh) = db().await;
        let provider = Arc::new(AlwaysProvider {
            turn: Box::new(|| Ok(ToolTurn::Final("ок".into()))),
        });
        // Бюджет на 1 спавн: первый ребёнок done, второй — budget-denied failed (батч всё равно отдаёт 2).
        let mut ctx = make_ctx(&dir, &dbh, provider).await;
        ctx.budget = DelegationBudget::new(1, 1, 3, Duration::from_secs(3600));
        let tool = DelegateTool::new(ctx, 3);
        let out = tool
            .invoke(r#"{"tasks":[{"goal":"a"},{"goal":"b"}]}"#)
            .await
            .expect("delegate ok даже при отказе ребёнка");
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        let arr = v.as_array().unwrap();
        assert_eq!(
            arr.len(),
            2,
            "батч завершился с двумя записями, не оборвался"
        );
        let statuses: Vec<&str> = arr.iter().map(|e| e["status"].as_str().unwrap()).collect();
        assert!(
            statuses.contains(&"done") && statuses.contains(&"failed"),
            "один done, один failed (бюджет на 1): {statuses:?}"
        );
    }
}
