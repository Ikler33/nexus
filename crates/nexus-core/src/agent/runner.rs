//! Цикл агента (AGENT-1): ограниченный, событие-стримящий луп `провайдер → инструменты → фенс → назад`.
//!
//! Контракт хода:
//! 1. Спросить tool-capable провайдера (`stream_chat_tools`) — контент стримится как [`AgentEvent::AssistantToken`].
//! 2. Эмитить [`AgentEvent::ContextUsage`] (токены текущих сообщений / окно из [`ContextBudget`]).
//! 3. Если ход = `Final` → эмит [`AgentEvent::Final`], выход `LoopOutcome::Final`.
//! 4. Если ход = `ToolCalls` → СНАЧАЛА дописать ОДНО сообщение роли `assistant` с `tool_calls`
//!    (строгий OpenAI-протокол), затем для КАЖДОГО вызова: эмит [`AgentEvent::ToolCall`] → диспатч через
//!    реестр → зафенсить результат (`fence_observation` + per-request `injection_marker`) → дописать как
//!    сообщение роли `"tool"` с `tool_call_id` (корреляция call↔result) → эмит [`AgentEvent::ToolResult`].
//!    Ошибка ОДНОГО инструмента → `ToolResult{is_error}` (модель восстанавливается), цикл НЕ падает.
//! 5. Повторять, пока: финал / `max_steps` / `wall_clock` / `cancel` / превышение токен-бюджета.
//!
//! Границы ([`LoopBounds`] + [`ContextBudget`]) → [`LoopOutcome::BudgetExhausted`] (без дальнейших
//! исполнений инструментов). Терминальный сбой провайдера → [`LoopOutcome::Error`].

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::ai::tools::{ToolCapableProvider, ToolTurn};
use crate::ai::{
    fence_observation, injection_marker, ChatMessage, ContextBudget, ToolCallFn, ToolCallMsg,
};
use crate::chunker::Tokenizer;
use crate::net::RunCtx;

use super::event::AgentEvent;
use super::registry::ToolRegistry;

/// Границы цикла (помимо токен-бюджета, который приходит из [`ContextBudget`]).
#[derive(Debug, Clone, Copy)]
pub struct LoopBounds {
    /// Максимум ходов модели (анти-зацикливание). Достигнут → [`BudgetKind::Steps`].
    pub max_steps: usize,
    /// Максимальное стенное время всего прогона. Истекло → [`BudgetKind::WallClock`].
    pub wall_clock: Duration,
}

impl Default for LoopBounds {
    /// Разумный дефолт: 8 ходов, 5 минут (как и щедрый потолок тика планировщика).
    fn default() -> Self {
        Self {
            max_steps: 8,
            wall_clock: Duration::from_secs(300),
        }
    }
}

/// Какая граница исчерпалась (для диагностики/UI).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BudgetKind {
    /// Достигнут `max_steps`.
    Steps,
    /// Истёк `wall_clock`.
    WallClock,
    /// Сообщения превысили токен-бюджет окна (входной бюджет [`ContextBudget::input_budget`]).
    Tokens,
    /// Прогон отменён (`cancel`).
    Cancelled,
    /// **KILL-SWITCH (AGENT-5): прогон остановлен глобальной паузой `agent_paused`.** Отдельный вид
    /// (не `Cancelled`): таксономия не врёт — это не отмена прогона, а пауза агента; хендлер ре-кьюит
    /// прогон, чтобы он возобновился на un-pause. Останов промптовый — на следующей проверке границы.
    Paused,
}

/// Исход цикла.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoopOutcome {
    /// Модель дала финальный ответ.
    Final(String),
    /// Исчерпана граница до финала. `partial` — последний контент модели (если был), чтобы UI показал хоть что-то.
    BudgetExhausted { kind: BudgetKind, partial: String },
    /// Терминальная ошибка (сбой провайдера / битый ход даже после ре-ask).
    Error(String),
}

/// Считает токены всех сообщений (контент + ChatML-оверхед на сообщение) — РОВНО та же формула, что
/// меряет [`ContextBudget`] при `fit` (через `pub(crate) ContextBudget::message_cost`). Один источник
/// cost-математики (B): без дубля константы оценки бюджета не разойдутся. Это `used` для
/// [`AgentEvent::ContextUsage`].
fn count_used(tk: &dyn Tokenizer, messages: &[ChatMessage]) -> usize {
    messages
        .iter()
        .map(|m| ContextBudget::message_cost(tk, m))
        .sum()
}

/// Запускает ограниченный цикл агента. См. модульный док — контракт хода.
///
/// `on_event` получает поток [`AgentEvent`] (UI-1 потребитель). `cancel` прерывает между/во время ходов.
/// Эгресс провайдера уже за [`crate::net::GuardedClient`] — цикл сети напрямую не касается.
///
/// `ctx` — run-контекст прогона (AGENT-3a): ЯВНО пробрасывается в КАЖДЫЙ ход провайдера, чтобы эгресс
/// этого прогона коррелировался на его `run_id` в audit (per-call, не процесс-глобальный слот —
/// конкурентные прогоны не перетирают атрибуцию друг друга). Вне прогона/в смоук-тестах — [`RunCtx::NONE`].
/// `agent_paused` — глобальный KILL-SWITCH агента (AGENT-5): процесс-глобальный `Arc<AtomicBool>`,
/// проверяемый fail-safe (взведён ⇒ НЕ действуем) на КАЖДОМ шаге РЯДОМ с `cancel`. Взведён мид-ран ⇒
/// цикл останавливается на следующей проверке границы → [`BudgetKind::Paused`] (не `Cancelled`,
/// не `Error`): хендлер ре-кьюит прогон для возобновления на un-pause. Это ВТОРОЙ из трёх чек-пойнтов
/// kill-switch (1-й — `drive` до старта оставляет прогон queued; 3-й — актуатор не пишет под паузой).
/// `paused_nanos` — **Fix BF-1 №1**: счётчик наносекунд, накопленных на ОЖИДАНИИ человеческого решения
/// у гейта (пишется в `decide()` — см. `session::PauseAccountingDecision`). Это время ВЫЧИТАЕТСЯ из
/// стенного возраста при проверке `wall_clock` (раздумья человека у changeset-гейта НЕ жгут бюджет).
/// Прямые вызыватели без гейта (smoke/eval/sandbox-child) передают свежий 0-счётчик (поведение прежнее).
#[allow(clippy::too_many_arguments)] // цикл явно принимает все свои зависимости (тестируемость > эргономика)
pub async fn run_agent_loop(
    provider: &dyn ToolCapableProvider,
    registry: &ToolRegistry,
    mut messages: Vec<ChatMessage>,
    bounds: LoopBounds,
    budget: &ContextBudget,
    tk: &dyn Tokenizer,
    cancel: &Arc<AtomicBool>,
    agent_paused: &Arc<AtomicBool>,
    paused_nanos: &Arc<AtomicU64>,
    ctx: RunCtx,
    on_event: &mut (dyn FnMut(AgentEvent) + Send),
) -> LoopOutcome {
    let specs = registry.specs();
    let start = Instant::now();
    let mut last_content = String::new();
    // Сколько ещё раз можно простить битый-JSON-ход одним capped re-ask. Ровно ОДИН (контракт SSE).
    let mut reask_budget: u32 = 1;

    for _step in 0..bounds.max_steps {
        // — границы ДО хода —
        // KILL-SWITCH (AGENT-5, чек-пойнт #2): глобальная пауза агента проверяется ПЕРВОЙ и fail-safe
        // (взведена ⇒ останов). Отдельный вид Paused (не Cancelled): хендлер ре-кьюит прогон для
        // возобновления на un-pause. Останов промптовый — здесь, ДО хода модели/диспатча инструментов:
        // взведённая мид-ран пауза не даст следующему ходу позвать ни модель, ни инструмент (значит и
        // ни одной записи актуатора из ЭТОГО цикла — третий чек-пойнт страхует уже-идущий ход).
        if agent_paused.load(Ordering::Relaxed) {
            return LoopOutcome::BudgetExhausted {
                kind: BudgetKind::Paused,
                partial: last_content,
            };
        }
        if cancel.load(Ordering::Relaxed) {
            return LoopOutcome::BudgetExhausted {
                kind: BudgetKind::Cancelled,
                partial: last_content,
            };
        }
        // Fix BF-1 №1: время ОЖИДАНИЯ человеческого решения у гейта (`paused_nanos`, копится в `decide()`)
        // ВЫЧИТАЕТСЯ из стенного возраста — раздумья человека НЕ жгут wall_clock-бюджет. saturating_sub:
        // пауза не может дать «отрицательный» возраст. Kill-switch/Cancelled выше — их семантика цела
        // (вычитается ТОЛЬКО блокировка на decide, не отмена/пауза прогона).
        let paused = Duration::from_nanos(paused_nanos.load(Ordering::Relaxed));
        if start.elapsed().saturating_sub(paused) >= bounds.wall_clock {
            // Fix BF-1 №2: жёсткий исход бюджета эмитит терминальный Error (иначе UI вечно «Выполняю…»).
            return emit_budget_exhausted(on_event, BudgetKind::WallClock, last_content);
        }
        // Токен-бюджет: сообщения не должны превышать ВХОДНОЙ бюджет окна (резерв под ответ оставлен).
        let used = count_used(tk, &messages);
        on_event(AgentEvent::ContextUsage {
            used,
            window: budget.context_window,
        });
        if used > budget.input_budget() {
            return emit_budget_exhausted(on_event, BudgetKind::Tokens, last_content);
        }

        // — ход модели (контент стримится как AssistantToken; параллельно копим его в `last_content`,
        //   чтобы `partial` при исчерпании границы нёс последний частичный вывод модели для UI) —
        let mut turn_content = String::new();
        let turn = {
            let mut on_token = |t: String| {
                turn_content.push_str(&t);
                on_event(AgentEvent::AssistantToken(t));
            };
            provider
                .stream_chat_tools(&messages, &specs, &mut on_token, cancel, ctx)
                .await
        };
        last_content = turn_content;

        let turn = match turn {
            Ok(t) => t,
            Err(e) => {
                // Битый ход (склей args не-JSON и т.п.): ровно ОДИН capped re-ask, затем ошибка хода.
                if reask_budget > 0 && is_reaskable(&e) {
                    reask_budget -= 1;
                    messages.push(ChatMessage::user(
                        "Предыдущий вызов инструмента был некорректным (аргументы — не валидный \
                         JSON). Повтори вызов с корректным JSON в аргументах.",
                    ));
                    continue;
                }
                let msg = e.to_string();
                on_event(AgentEvent::Error(msg.clone()));
                return LoopOutcome::Error(msg);
            }
        };

        match turn {
            ToolTurn::Final(content) => {
                // Финал: контент уже стримился токенами; эмитим явный Final с полным текстом.
                on_event(AgentEvent::Final(content.clone()));
                return LoopOutcome::Final(content);
            }
            ToolTurn::ToolCalls(calls) => {
                // Строгий OpenAI tool-протокол (A): ПЕРЕД tool-результатами дописываем ОДНО сообщение
                // роли `assistant` с накопленными `tool_calls` (id+name+arguments). Так массив сообщений
                // спек-совместим, а tool_call_id результата коррелирует с конкретным вызовом (не теряем
                // соответствие при нескольких вызовах в одном ходу).
                messages.push(ChatMessage::assistant_tool_calls(
                    calls
                        .iter()
                        .map(|c| ToolCallMsg {
                            id: c.id.clone(),
                            kind: "function".into(),
                            function: ToolCallFn {
                                name: c.name.clone(),
                                arguments: c.arguments.clone(),
                            },
                        })
                        .collect(),
                ));
                // Исполняем КАЖДЫЙ вызов: событие ДО, диспатч, фенс, дописать tool-сообщение, событие ПОСЛЕ.
                for call in &calls {
                    on_event(AgentEvent::ToolCall {
                        id: call.id.clone(),
                        kind: call.name.clone(),
                        args: call.arguments.clone(),
                    });
                    let result = registry.dispatch(call).await;
                    // Фенсим РЕЗУЛЬТАТ при ре-инъекции в промпт (per-request marker, как RAG/observation):
                    // tool-output — недоверенные ДАННЫЕ, не инструкции (I-5/AC-SEC-7). Корреляцию с вызовом
                    // несёт `tool_call_id` (роль `tool`), а не позиция в массиве.
                    let marker = injection_marker();
                    let fenced = fence_observation("tool", &result.content, &marker);
                    messages.push(ChatMessage::tool(call.id.clone(), fenced));
                    on_event(AgentEvent::ToolResult {
                        id: result.id,
                        content: result.content,
                        is_error: result.is_error,
                    });
                }
                // следующий ход продолжит с дополненными сообщениями (tool-результаты в истории)
            }
        }
    }

    // Исчерпан max_steps без финала.
    emit_budget_exhausted(on_event, BudgetKind::Steps, last_content)
}

/// Можно ли простить этот сбой одним re-ask. Только «битый ответ модели» (склей args не-JSON / ход без
/// валидных вызовов) — это [`crate::ai::AiError::BadResponse`]. Сетевые/политические/таймаут — нет.
fn is_reaskable(e: &crate::ai::AiError) -> bool {
    matches!(e, crate::ai::AiError::BadResponse(_))
}

/// **Fix BF-1 №2 — ЕДИНЫЙ текст «бюджет исчерпан»** для Steps/WallClock/Tokens. Один источник для
/// стрим-события цикла ([`emit_budget_exhausted`]) И финализации run_store
/// ([`super::finish::outcome_to_finish`]) — чтобы UI (терминал стрима) и история прогона (запись в БД)
/// НЕ расходились. `Paused`/`Cancelled` имеют СВОИ тексты (у финализатора) и через этот хелпер НЕ идут.
pub(crate) fn budget_exhausted_text(kind: BudgetKind, partial: &str) -> String {
    format!("бюджет исчерпан ({kind:?}); частичный ответ: {partial}")
}

/// **Fix BF-1 №2**: собрать `BudgetExhausted`-исход ЖЁСТКОГО бюджета (Steps/WallClock/Tokens) И эмитить
/// терминальное [`AgentEvent::Error`] в поток. До фикса эти исходы не слали НИ ОДНОГО терминального
/// события → one-shot вызыватели (desktop/cli/acp/connect) не уводили UI из «Выполняю…». Одно место
/// чинит всех. Текст — из [`budget_exhausted_text`] (совпадает с записью в БД). `Paused` (agentd
/// паркует/ре-кьюит — НЕ терминал) и `Cancelled` (финализирует вызыватель) сюда НЕ передаются.
fn emit_budget_exhausted(
    on_event: &mut (dyn FnMut(AgentEvent) + Send),
    kind: BudgetKind,
    partial: String,
) -> LoopOutcome {
    debug_assert!(
        matches!(
            kind,
            BudgetKind::Steps | BudgetKind::WallClock | BudgetKind::Tokens
        ),
        "emit_budget_exhausted только для жёсткого бюджета (не Paused/Cancelled)"
    );
    on_event(AgentEvent::Error(budget_exhausted_text(kind, &partial)));
    LoopOutcome::BudgetExhausted { kind, partial }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::stubs::EchoTool;
    use crate::agent::tool::ToolCall;
    use crate::ai::AiResult;
    use crate::chunker::WordTokenizer;
    use async_trait::async_trait;
    use std::sync::Mutex;

    /// Фейковый tool-capable провайдер: отдаёт заранее заданную последовательность ходов (по одному на
    /// вызов). Доказывает цикл БЕЗ сети. Опционально стримит контент в on_token каждого хода.
    struct FakeToolProvider {
        turns: Mutex<std::collections::VecDeque<ScriptedTurn>>,
        calls_seen: Mutex<usize>,
        /// Снимок `messages`, переданных провайдеру на КАЖДОМ ходу (для проверки строгой формы A).
        seen_messages: Mutex<Vec<Vec<ChatMessage>>>,
    }

    /// Сценарный ход: что вернуть + что «настримить» как токены.
    struct ScriptedTurn {
        stream: Option<String>,
        result: AiResult<ToolTurn>,
    }

    impl FakeToolProvider {
        fn new(turns: Vec<ScriptedTurn>) -> Self {
            Self {
                turns: Mutex::new(turns.into_iter().collect()),
                calls_seen: Mutex::new(0),
                seen_messages: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait]
    impl ToolCapableProvider for FakeToolProvider {
        async fn stream_chat_tools(
            &self,
            messages: &[ChatMessage],
            _tools: &[crate::agent::tool::ToolSpec],
            on_token: &mut (dyn FnMut(String) + Send),
            _cancel: &Arc<AtomicBool>,
            _ctx: RunCtx,
        ) -> AiResult<ToolTurn> {
            *self.calls_seen.lock().unwrap() += 1;
            self.seen_messages.lock().unwrap().push(messages.to_vec());
            let turn = self
                .turns
                .lock()
                .unwrap()
                .pop_front()
                .expect("FakeToolProvider: ходы исчерпаны (цикл не остановился вовремя?)");
            if let Some(s) = turn.stream {
                on_token(s);
            }
            turn.result
        }
        fn model_id(&self) -> &str {
            "fake"
        }
    }

    fn echo_call(id: &str, text: &str) -> ToolCall {
        ToolCall {
            id: id.into(),
            name: "debug.echo".into(),
            arguments: format!(r#"{{"text":{}}}"#, serde_json::Value::String(text.into())),
        }
    }

    /// Инструмент-счётчик исполнений (для доказательства «НИЧЕГО не диспатчится» при битом ходе):
    /// каждый `invoke` инкрементит общий счётчик. Имя `debug.count`.
    struct CountingTool(Arc<std::sync::atomic::AtomicUsize>);

    #[async_trait]
    impl crate::agent::tool::Tool for CountingTool {
        fn spec(&self) -> crate::agent::tool::ToolSpec {
            crate::agent::tool::ToolSpec {
                name: "debug.count".into(),
                description: "счётчик исполнений (тест)".into(),
                parameters: serde_json::json!({"type":"object"}),
            }
        }
        async fn invoke(&self, _args: &str) -> Result<String, crate::agent::tool::ToolError> {
            self.0.fetch_add(1, Ordering::Relaxed);
            Ok("counted".into())
        }
    }

    /// **Fix BF-1 №1 (тест)**: инструмент, чей `invoke` БЛОКИРУЕТСЯ `delay` (эмулируя ожидание
    /// человеческого решения у гейта) и — если задан `paused_nanos` — записывает эту длительность в
    /// счётчик пауз (ровно как [`crate::agent::session::PauseAccountingDecision`] вокруг `decide()`). Так
    /// wall_clock-вычитание паузы доказывается на уровне цикла, БЕЗ поднятия всего актуатора. Имя `debug.pause`.
    struct PausingTool {
        delay: Duration,
        paused_nanos: Option<Arc<AtomicU64>>,
    }

    #[async_trait]
    impl crate::agent::tool::Tool for PausingTool {
        fn spec(&self) -> crate::agent::tool::ToolSpec {
            crate::agent::tool::ToolSpec {
                name: "debug.pause".into(),
                description: "спит delay (эмуляция ожидания решения)".into(),
                parameters: serde_json::json!({"type":"object"}),
            }
        }
        async fn invoke(&self, _args: &str) -> Result<String, crate::agent::tool::ToolError> {
            // Кредитуем ИЗМЕРЕННОЕ время сна (не константу delay) — как реальный декоратор мерит
            // фактическую блокировку на decide(). На нагруженном CI sleep может занять дольше delay;
            // константа оставила бы некредитованный хвост → флейк wall_clock-теста.
            let t0 = Instant::now();
            tokio::time::sleep(self.delay).await;
            if let Some(p) = &self.paused_nanos {
                let elapsed = u64::try_from(t0.elapsed().as_nanos()).unwrap_or(u64::MAX);
                p.fetch_add(elapsed, Ordering::Relaxed);
            }
            Ok("paused".into())
        }
    }

    fn pause_call(id: &str) -> ToolCall {
        ToolCall {
            id: id.into(),
            name: "debug.pause".into(),
            arguments: "{}".into(),
        }
    }

    fn budget() -> ContextBudget {
        ContextBudget {
            context_window: 100_000,
            reserve_output: 1024,
        }
    }

    /// КЛЮЧЕВОЕ ДОКАЗАТЕЛЬСТВО (offline mocked-loop): провайдер возвращает ToolCalls([echo]) на ходу 1,
    /// Final("done") на ходу 2. Цикл ОБЯЗАН: эмитить ToolCall→ToolResult (зафенсенный, коррелирован по
    /// id), дописать сообщение роли "tool", скормить обратно, завершиться Final("done"); поток событий
    /// содержит ToolCall→ToolResult→Final по порядку + хотя бы один ContextUsage.
    #[tokio::test]
    async fn mocked_loop_executes_feeds_back_and_finals() {
        let provider = FakeToolProvider::new(vec![
            ScriptedTurn {
                stream: Some("сейчас проверю".into()),
                result: Ok(ToolTurn::ToolCalls(vec![echo_call("call_1", "привет")])),
            },
            ScriptedTurn {
                stream: None,
                result: Ok(ToolTurn::Final("done".into())),
            },
        ]);
        let mut reg = ToolRegistry::new();
        reg.insert(Arc::new(EchoTool));
        let tk = WordTokenizer;
        let cancel = Arc::new(AtomicBool::new(false));
        let agent_paused = Arc::new(AtomicBool::new(false));
        let mut events: Vec<AgentEvent> = Vec::new();

        let outcome = run_agent_loop(
            &provider,
            &reg,
            vec![ChatMessage::user("позови echo с 'привет'")],
            LoopBounds::default(),
            &budget(),
            &tk,
            &cancel,
            &agent_paused,
            &Arc::new(AtomicU64::new(0)),
            RunCtx::NONE,
            &mut |e| events.push(e),
        )
        .await;

        // Терминал — Final("done").
        assert_eq!(outcome, LoopOutcome::Final("done".into()));

        // Поток содержит ContextUsage хотя бы раз.
        assert!(
            events
                .iter()
                .any(|e| matches!(e, AgentEvent::ContextUsage { .. })),
            "должен быть ContextUsage"
        );

        // Позиции ключевых событий: ToolCall → ToolResult → Final, по порядку.
        let pos = |pred: &dyn Fn(&AgentEvent) -> bool| events.iter().position(pred);
        let p_call =
            pos(&|e| matches!(e, AgentEvent::ToolCall { kind, .. } if kind == "debug.echo"))
                .expect("есть ToolCall echo");
        let p_res = pos(&|e| matches!(e, AgentEvent::ToolResult { .. })).expect("есть ToolResult");
        let p_final = pos(&|e| matches!(e, AgentEvent::Final(_))).expect("есть Final");
        assert!(p_call < p_res, "ToolCall раньше ToolResult");
        assert!(p_res < p_final, "ToolResult раньше Final");

        // Корреляция по id и содержимое результата (echo вернул аргумент).
        let (call_id, res_id, res_content, is_err) = {
            let mut ci = None;
            let mut ri = None;
            let mut rc = String::new();
            let mut er = true;
            for e in &events {
                match e {
                    AgentEvent::ToolCall { id, .. } => ci = Some(id.clone()),
                    AgentEvent::ToolResult {
                        id,
                        content,
                        is_error,
                    } => {
                        ri = Some(id.clone());
                        rc = content.clone();
                        er = *is_error;
                    }
                    _ => {}
                }
            }
            (ci.unwrap(), ri.unwrap(), rc, er)
        };
        assert_eq!(call_id, "call_1");
        assert_eq!(res_id, "call_1", "ToolResult коррелирован с ToolCall по id");
        assert!(!is_err, "echo не ошибка");
        assert!(res_content.contains("привет"), "echo вернул аргумент");

        // Контент первого хода стримился как AssistantToken.
        assert!(events
            .iter()
            .any(|e| matches!(e, AgentEvent::AssistantToken(s) if s.contains("проверю"))));

        // Провайдер вызван ровно 2 раза (ход с инструментом + финал) — feed-back состоялся.
        assert_eq!(*provider.calls_seen.lock().unwrap(), 2);

        // A (строгий OpenAI-протокол): на ВТОРОМ ходу модель видит СПЕК-последовательность —
        // user → assistant{tool_calls} → tool{tool_call_id}. Проверяем снимок сообщений второго вызова.
        let seen = provider.seen_messages.lock().unwrap();
        let second = &seen[1];
        let asst = second
            .iter()
            .find(|m| m.role == "assistant" && m.tool_calls.is_some())
            .expect("есть assistant{tool_calls} перед tool-результатом");
        let calls = asst.tool_calls.as_ref().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id, "call_1");
        assert_eq!(calls[0].kind, "function");
        assert_eq!(calls[0].function.name, "debug.echo");
        // Сообщение-результат: роль tool, коррелирует по tool_call_id с тем же id, и НЕ несёт tool_calls.
        let tool_msg = second
            .iter()
            .find(|m| m.role == "tool")
            .expect("есть tool-сообщение");
        assert_eq!(tool_msg.tool_call_id.as_deref(), Some("call_1"));
        assert!(tool_msg.tool_calls.is_none());
        // Порядок: assistant{tool_calls} строго ПЕРЕД tool-результатом (спека OpenAI).
        let p_asst = second
            .iter()
            .position(|m| m.role == "assistant" && m.tool_calls.is_some())
            .unwrap();
        let p_tool = second.iter().position(|m| m.role == "tool").unwrap();
        assert!(
            p_asst < p_tool,
            "assistant{{tool_calls}} раньше tool-результата"
        );
    }

    /// Провайдер ВСЕГДА возвращает ToolCalls → цикл упирается в max_steps → BudgetExhausted{Steps}.
    #[tokio::test]
    async fn loop_hits_max_steps() {
        let turns: Vec<ScriptedTurn> = (0..10)
            .map(|i| ScriptedTurn {
                stream: None,
                result: Ok(ToolTurn::ToolCalls(vec![echo_call(
                    &format!("c{i}"),
                    "loop",
                )])),
            })
            .collect();
        let provider = FakeToolProvider::new(turns);
        let mut reg = ToolRegistry::new();
        reg.insert(Arc::new(EchoTool));
        let tk = WordTokenizer;
        let cancel = Arc::new(AtomicBool::new(false));
        let agent_paused = Arc::new(AtomicBool::new(false));
        let bounds = LoopBounds {
            max_steps: 3,
            wall_clock: Duration::from_secs(60),
        };
        let outcome = run_agent_loop(
            &provider,
            &reg,
            vec![ChatMessage::user("loop")],
            bounds,
            &budget(),
            &tk,
            &cancel,
            &agent_paused,
            &Arc::new(AtomicU64::new(0)),
            RunCtx::NONE,
            &mut |_| {},
        )
        .await;
        assert!(
            matches!(
                outcome,
                LoopOutcome::BudgetExhausted {
                    kind: BudgetKind::Steps,
                    ..
                }
            ),
            "ожидали BudgetExhausted{{Steps}}: {outcome:?}"
        );
        // Ровно max_steps ходов модели — не больше.
        assert_eq!(*provider.calls_seen.lock().unwrap(), 3);
    }

    /// Final на первом же ходу → немедленный возврат, инструменты не трогаются.
    #[tokio::test]
    async fn loop_finals_immediately() {
        let provider = FakeToolProvider::new(vec![ScriptedTurn {
            stream: Some("ответ".into()),
            result: Ok(ToolTurn::Final("сразу финал".into())),
        }]);
        let reg = ToolRegistry::new();
        let tk = WordTokenizer;
        let cancel = Arc::new(AtomicBool::new(false));
        let agent_paused = Arc::new(AtomicBool::new(false));
        let mut events = Vec::new();
        let outcome = run_agent_loop(
            &provider,
            &reg,
            vec![ChatMessage::user("q")],
            LoopBounds::default(),
            &budget(),
            &tk,
            &cancel,
            &agent_paused,
            &Arc::new(AtomicU64::new(0)),
            RunCtx::NONE,
            &mut |e| events.push(e),
        )
        .await;
        assert_eq!(outcome, LoopOutcome::Final("сразу финал".into()));
        assert!(!events
            .iter()
            .any(|e| matches!(e, AgentEvent::ToolCall { .. })));
        assert_eq!(*provider.calls_seen.lock().unwrap(), 1);
    }

    /// wall_clock=0 → срабатывает на ПЕРВОЙ же проверке границы (до любого хода модели).
    #[tokio::test]
    async fn loop_trips_wall_clock() {
        let provider = FakeToolProvider::new(vec![]); // не должен быть вызван
        let reg = ToolRegistry::new();
        let tk = WordTokenizer;
        let cancel = Arc::new(AtomicBool::new(false));
        let agent_paused = Arc::new(AtomicBool::new(false));
        let bounds = LoopBounds {
            max_steps: 5,
            wall_clock: Duration::ZERO,
        };
        let outcome = run_agent_loop(
            &provider,
            &reg,
            vec![ChatMessage::user("q")],
            bounds,
            &budget(),
            &tk,
            &cancel,
            &agent_paused,
            &Arc::new(AtomicU64::new(0)),
            RunCtx::NONE,
            &mut |_| {},
        )
        .await;
        assert!(matches!(
            outcome,
            LoopOutcome::BudgetExhausted {
                kind: BudgetKind::WallClock,
                ..
            }
        ));
        assert_eq!(*provider.calls_seen.lock().unwrap(), 0);
    }

    /// Ошибка инструмента (неизвестное имя) → ToolResult{is_error} скормлен обратно, цикл НЕ падает,
    /// доходит до Final следующего хода.
    #[tokio::test]
    async fn tool_error_is_fed_back_not_fatal() {
        let provider = FakeToolProvider::new(vec![
            ScriptedTurn {
                stream: None,
                result: Ok(ToolTurn::ToolCalls(vec![ToolCall {
                    id: "x".into(),
                    name: "does.not.exist".into(),
                    arguments: "{}".into(),
                }])),
            },
            ScriptedTurn {
                stream: None,
                result: Ok(ToolTurn::Final("восстановился".into())),
            },
        ]);
        let reg = ToolRegistry::new(); // пустой → инструмент неизвестен
        let tk = WordTokenizer;
        let cancel = Arc::new(AtomicBool::new(false));
        let agent_paused = Arc::new(AtomicBool::new(false));
        let mut events = Vec::new();
        let outcome = run_agent_loop(
            &provider,
            &reg,
            vec![ChatMessage::user("q")],
            LoopBounds::default(),
            &budget(),
            &tk,
            &cancel,
            &agent_paused,
            &Arc::new(AtomicU64::new(0)),
            RunCtx::NONE,
            &mut |e| events.push(e),
        )
        .await;
        assert_eq!(outcome, LoopOutcome::Final("восстановился".into()));
        assert!(
            events
                .iter()
                .any(|e| matches!(e, AgentEvent::ToolResult { is_error, .. } if *is_error)),
            "ошибочный ToolResult скормлен обратно"
        );
    }

    /// Битый ход (BadResponse) → ровно ОДИН re-ask, затем (повтор битого) ошибка цикла.
    #[tokio::test]
    async fn loop_reasks_once_then_errors() {
        use crate::ai::AiError;
        let provider = FakeToolProvider::new(vec![
            ScriptedTurn {
                stream: None,
                result: Err(AiError::BadResponse("битый JSON".into())),
            },
            ScriptedTurn {
                stream: None,
                result: Err(AiError::BadResponse("снова битый".into())),
            },
        ]);
        let reg = ToolRegistry::new();
        let tk = WordTokenizer;
        let cancel = Arc::new(AtomicBool::new(false));
        let agent_paused = Arc::new(AtomicBool::new(false));
        let mut events = Vec::new();
        let outcome = run_agent_loop(
            &provider,
            &reg,
            vec![ChatMessage::user("q")],
            LoopBounds::default(),
            &budget(),
            &tk,
            &cancel,
            &agent_paused,
            &Arc::new(AtomicU64::new(0)),
            RunCtx::NONE,
            &mut |e| events.push(e),
        )
        .await;
        assert!(matches!(outcome, LoopOutcome::Error(_)));
        // Провайдер вызван дважды: исходный + один re-ask.
        assert_eq!(*provider.calls_seen.lock().unwrap(), 2);
        assert!(events.iter().any(|e| matches!(e, AgentEvent::Error(_))));
    }

    /// Re-ask edge (D): ход с НЕВАЛИДНЫМИ args отвергается провайдером на finalize() ДО построения
    /// ToolTurn (см. tools.rs) → весь ход = BadResponse. Цикл ОБЯЗАН: сделать РОВНО ОДИН re-ask и
    /// НЕ диспатчить НИЧЕГО из этого хода (даже «хорошие» вызовы того же хода не исполняются — finalize
    /// валит ход целиком, а не по-вызову). Доказываем счётчиком исполнений = 0 на ходу с битыми args.
    #[tokio::test]
    async fn invalid_args_turn_reasks_once_and_dispatches_nothing() {
        use crate::ai::AiError;
        let dispatched = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        // Ход 1: битый-args ход (как вернул бы реальный finalize при невалидном JSON одного из вызовов)
        //         → BadResponse, НИ один вызов не дошёл до диспатча. Ход 2 (re-ask): валидный финал.
        let provider = FakeToolProvider::new(vec![
            ScriptedTurn {
                stream: None,
                result: Err(AiError::BadResponse(
                    "tool_call[1] 't': аргументы не JSON".into(),
                )),
            },
            ScriptedTurn {
                stream: None,
                result: Ok(ToolTurn::Final("исправился".into())),
            },
        ]);
        let mut reg = ToolRegistry::new();
        reg.insert(Arc::new(EchoTool));
        reg.insert(Arc::new(CountingTool(dispatched.clone())));
        let tk = WordTokenizer;
        let cancel = Arc::new(AtomicBool::new(false));
        let agent_paused = Arc::new(AtomicBool::new(false));
        let mut events = Vec::new();
        let outcome = run_agent_loop(
            &provider,
            &reg,
            vec![ChatMessage::user("позови инструменты")],
            LoopBounds::default(),
            &budget(),
            &tk,
            &cancel,
            &agent_paused,
            &Arc::new(AtomicU64::new(0)),
            RunCtx::NONE,
            &mut |e| events.push(e),
        )
        .await;
        // Ровно один re-ask привёл к финалу следующего хода.
        assert_eq!(outcome, LoopOutcome::Final("исправился".into()));
        // Провайдер вызван дважды: битый ход + один re-ask (НЕ больше — re-ask строго один).
        assert_eq!(*provider.calls_seen.lock().unwrap(), 2);
        // НИЧЕГО не исполнено из битого хода: 0 диспатчей (ни «хороший», ни «плохой» вызов).
        assert_eq!(
            dispatched.load(Ordering::Relaxed),
            0,
            "битый-args ход не должен исполнить НИ ОДИН инструмент (нет частичного исполнения)"
        );
        // И в потоке событий битого хода нет ни ToolCall, ни ToolResult (диспатча не было вовсе).
        assert!(
            !events.iter().any(|e| matches!(
                e,
                AgentEvent::ToolCall { .. } | AgentEvent::ToolResult { .. }
            )),
            "битый ход не эмитит ToolCall/ToolResult"
        );
    }

    /// cancel взведён до старта → BudgetExhausted{Cancelled}, провайдер не вызван.
    #[tokio::test]
    async fn loop_respects_cancel() {
        let provider = FakeToolProvider::new(vec![]);
        let reg = ToolRegistry::new();
        let tk = WordTokenizer;
        let cancel = Arc::new(AtomicBool::new(true));
        let agent_paused = Arc::new(AtomicBool::new(false));
        let outcome = run_agent_loop(
            &provider,
            &reg,
            vec![ChatMessage::user("q")],
            LoopBounds::default(),
            &budget(),
            &tk,
            &cancel,
            &agent_paused,
            &Arc::new(AtomicU64::new(0)),
            RunCtx::NONE,
            &mut |_| {},
        )
        .await;
        assert!(matches!(
            outcome,
            LoopOutcome::BudgetExhausted {
                kind: BudgetKind::Cancelled,
                ..
            }
        ));
        assert_eq!(*provider.calls_seen.lock().unwrap(), 0);
    }

    /// **KILL-SWITCH чек-пойнт #2 (взведён ДО старта)**: `agent_paused=true` до первого хода →
    /// BudgetExhausted{Paused}, провайдер НЕ вызван (ни одного хода модели, значит ни одного диспатча
    /// инструмента → ни одной записи актуатора из цикла).
    #[tokio::test]
    async fn loop_paused_before_start_does_not_run() {
        let provider = FakeToolProvider::new(vec![]); // не должен быть вызван
        let reg = ToolRegistry::new();
        let tk = WordTokenizer;
        let cancel = Arc::new(AtomicBool::new(false));
        let agent_paused = Arc::new(AtomicBool::new(true)); // ПАУЗА взведена
        let outcome = run_agent_loop(
            &provider,
            &reg,
            vec![ChatMessage::user("q")],
            LoopBounds::default(),
            &budget(),
            &tk,
            &cancel,
            &agent_paused,
            &Arc::new(AtomicU64::new(0)),
            RunCtx::NONE,
            &mut |_| {},
        )
        .await;
        assert!(
            matches!(
                outcome,
                LoopOutcome::BudgetExhausted {
                    kind: BudgetKind::Paused,
                    ..
                }
            ),
            "пауза до старта → Paused: {outcome:?}"
        );
        assert_eq!(
            *provider.calls_seen.lock().unwrap(),
            0,
            "под паузой провайдер (и значит инструменты) не вызывается"
        );
    }

    /// **KILL-SWITCH чек-пойнт #2 (взведён МИД-РАН)**: первый ход — ToolCalls(echo), ВО ВРЕМЯ on_event
    /// взводим паузу → цикл ОБЯЗАН остановиться на СЛЕДУЮЩЕЙ проверке границы (до второго хода
    /// модели/диспатча). Доказываем: провайдер вызван РОВНО 1 раз (второго хода не было), исход Paused,
    /// при этом инструмент ПЕРВОГО (уже-идущего) хода исполнился (счётчик >0) — пауза останавливает
    /// ДАЛЬНЕЙШИЕ ходы, а третий чек-пойнт (актуатор) страхует записи уже-идущего хода.
    #[tokio::test]
    async fn loop_paused_mid_run_stops_at_next_step() {
        // Ход 1: ToolCalls(echo). Ход 2: Final — НЕ должен быть достигнут (пауза остановит до него).
        let provider = FakeToolProvider::new(vec![
            ScriptedTurn {
                stream: None,
                result: Ok(ToolTurn::ToolCalls(vec![echo_call("c1", "x")])),
            },
            ScriptedTurn {
                stream: None,
                result: Ok(ToolTurn::Final("не достигнут".into())),
            },
        ]);
        let mut reg = ToolRegistry::new();
        reg.insert(Arc::new(EchoTool));
        let tk = WordTokenizer;
        let cancel = Arc::new(AtomicBool::new(false));
        let agent_paused = Arc::new(AtomicBool::new(false));
        // Взводим паузу, как только увидим РЕЗУЛЬТАТ инструмента первого хода (т.е. ход 1 уже идёт).
        let paused_for_cb = agent_paused.clone();
        let mut on_event = move |e: AgentEvent| {
            if matches!(e, AgentEvent::ToolResult { .. }) {
                paused_for_cb.store(true, Ordering::Relaxed);
            }
        };
        let outcome = run_agent_loop(
            &provider,
            &reg,
            vec![ChatMessage::user("позови echo")],
            LoopBounds::default(),
            &budget(),
            &tk,
            &cancel,
            &agent_paused,
            &Arc::new(AtomicU64::new(0)),
            RunCtx::NONE,
            &mut on_event,
        )
        .await;
        assert!(
            matches!(
                outcome,
                LoopOutcome::BudgetExhausted {
                    kind: BudgetKind::Paused,
                    ..
                }
            ),
            "пауза мид-ран → Paused на следующей проверке границы: {outcome:?}"
        );
        assert_eq!(
            *provider.calls_seen.lock().unwrap(),
            1,
            "ровно ОДИН ход модели — второй ход не стартовал (пауза остановила цикл)"
        );
    }

    /// Токен-бюджет: крошечное окно → used превышает input_budget → BudgetExhausted{Tokens} до хода.
    #[tokio::test]
    async fn loop_trips_token_budget() {
        let provider = FakeToolProvider::new(vec![]);
        let reg = ToolRegistry::new();
        let tk = WordTokenizer;
        let cancel = Arc::new(AtomicBool::new(false));
        let agent_paused = Arc::new(AtomicBool::new(false));
        let tiny = ContextBudget {
            context_window: 5,
            reserve_output: 4,
        }; // input_budget = 1
        let outcome = run_agent_loop(
            &provider,
            &reg,
            vec![ChatMessage::user(
                "это сообщение заведомо длиннее одного токена бюджета",
            )],
            LoopBounds::default(),
            &tiny,
            &tk,
            &cancel,
            &agent_paused,
            &Arc::new(AtomicU64::new(0)),
            RunCtx::NONE,
            &mut |_| {},
        )
        .await;
        assert!(matches!(
            outcome,
            LoopOutcome::BudgetExhausted {
                kind: BudgetKind::Tokens,
                ..
            }
        ));
        assert_eq!(*provider.calls_seen.lock().unwrap(), 0);
    }

    /// **Fix BF-1 №1 (ключевое доказательство)**: время ожидания человеческого решения у гейта
    /// (записанное в `paused_nanos`, как это делает `PauseAccountingDecision` вокруг `decide()`) НЕ тикает
    /// против wall_clock. wall_clock=20мс; инструмент «ждёт решения» 60мс и кредитует это как паузу. На
    /// СЛЕДУЮЩЕЙ проверке границы стенной возраст ~60мс, но `60−60 ≈ 0 < 20` ⇒ НЕ WallClock ⇒ цикл
    /// доходит до Final. Без вычитания тот же прогон умер бы по WallClock (см. control-тест ниже).
    #[tokio::test]
    async fn wall_clock_excludes_decision_wait_time() {
        let provider = FakeToolProvider::new(vec![
            ScriptedTurn {
                stream: None,
                result: Ok(ToolTurn::ToolCalls(vec![pause_call("p1")])),
            },
            ScriptedTurn {
                stream: None,
                result: Ok(ToolTurn::Final("готово".into())),
            },
        ]);
        let paused_nanos = Arc::new(AtomicU64::new(0));
        let mut reg = ToolRegistry::new();
        reg.insert(Arc::new(PausingTool {
            delay: Duration::from_millis(60),
            paused_nanos: Some(paused_nanos.clone()),
        }));
        let tk = WordTokenizer;
        let cancel = Arc::new(AtomicBool::new(false));
        let agent_paused = Arc::new(AtomicBool::new(false));
        let bounds = LoopBounds {
            max_steps: 5,
            wall_clock: Duration::from_millis(20),
        };
        let outcome = run_agent_loop(
            &provider,
            &reg,
            vec![ChatMessage::user("работай")],
            bounds,
            &budget(),
            &tk,
            &cancel,
            &agent_paused,
            &paused_nanos,
            RunCtx::NONE,
            &mut |_| {},
        )
        .await;
        assert_eq!(
            outcome,
            LoopOutcome::Final("готово".into()),
            "ожидание решения у гейта НЕ должно убивать прогон по WallClock: {outcome:?}"
        );
    }

    /// **Fix BF-1 №1 (control)**: тот же сценарий, но время инструмента НЕ кредитуется как пауза (обычная
    /// работа, не ожидание решения). Стенной возраст ~60мс ≥ 20мс, paused=0 ⇒ WallClock срабатывает на
    /// следующей границе. Доказывает, что вычитается ИМЕННО пауза — жёсткий бюджет по-прежнему стережёт.
    #[tokio::test]
    async fn wall_clock_still_trips_when_time_not_credited_as_pause() {
        let provider = FakeToolProvider::new(vec![
            ScriptedTurn {
                stream: None,
                result: Ok(ToolTurn::ToolCalls(vec![pause_call("p1")])),
            },
            ScriptedTurn {
                stream: None,
                result: Ok(ToolTurn::Final("не достигнут".into())),
            },
        ]);
        let paused_nanos = Arc::new(AtomicU64::new(0));
        let mut reg = ToolRegistry::new();
        reg.insert(Arc::new(PausingTool {
            delay: Duration::from_millis(60),
            paused_nanos: None,
        }));
        let tk = WordTokenizer;
        let cancel = Arc::new(AtomicBool::new(false));
        let agent_paused = Arc::new(AtomicBool::new(false));
        let bounds = LoopBounds {
            max_steps: 5,
            wall_clock: Duration::from_millis(20),
        };
        let outcome = run_agent_loop(
            &provider,
            &reg,
            vec![ChatMessage::user("работай")],
            bounds,
            &budget(),
            &tk,
            &cancel,
            &agent_paused,
            &paused_nanos,
            RunCtx::NONE,
            &mut |_| {},
        )
        .await;
        assert!(
            matches!(
                outcome,
                LoopOutcome::BudgetExhausted {
                    kind: BudgetKind::WallClock,
                    ..
                }
            ),
            "без учёта паузы жёсткий wall_clock обязан сработать: {outcome:?}"
        );
    }

    /// **Fix BF-1 №2**: жёсткий исход бюджета (здесь WallClock) эмитит РОВНО ОДНО терминальное
    /// [`AgentEvent::Error`] в поток — иначе one-shot UI (desktop/cli/acp/connect) вечно «Выполняю…».
    /// Текст события СОВПАДАЕТ с записью в БД ([`budget_exhausted_text`] = `finish::outcome_to_finish`).
    #[tokio::test]
    async fn hard_budget_exhaustion_emits_terminal_error_once() {
        let provider = FakeToolProvider::new(vec![]);
        let reg = ToolRegistry::new();
        let paused_nanos = Arc::new(AtomicU64::new(0));
        let tk = WordTokenizer;
        let cancel = Arc::new(AtomicBool::new(false));
        let agent_paused = Arc::new(AtomicBool::new(false));
        let bounds = LoopBounds {
            max_steps: 5,
            wall_clock: Duration::ZERO,
        };
        let mut events = Vec::new();
        let outcome = run_agent_loop(
            &provider,
            &reg,
            vec![ChatMessage::user("q")],
            bounds,
            &budget(),
            &tk,
            &cancel,
            &agent_paused,
            &paused_nanos,
            RunCtx::NONE,
            &mut |e| events.push(e),
        )
        .await;
        assert!(matches!(
            outcome,
            LoopOutcome::BudgetExhausted {
                kind: BudgetKind::WallClock,
                ..
            }
        ));
        let errs: Vec<&String> = events
            .iter()
            .filter_map(|e| match e {
                AgentEvent::Error(m) => Some(m),
                _ => None,
            })
            .collect();
        assert_eq!(
            errs.len(),
            1,
            "ровно ОДНО терминальное Error-событие: {events:?}"
        );
        assert_eq!(
            errs[0],
            &budget_exhausted_text(BudgetKind::WallClock, ""),
            "текст события совпадает с записью в БД (finish::outcome_to_finish)"
        );
    }

    /// **Fix BF-1 №2 (граница)**: `Paused` (kill-switch) — НЕ терминал (agentd паркует/ре-кьюит прогон)
    /// ⇒ НИ ОДНОГО Error-события (иначе UI показал бы ошибку у прогона, который лишь приостановлен).
    #[tokio::test]
    async fn paused_outcome_does_not_emit_error() {
        let provider = FakeToolProvider::new(vec![]);
        let reg = ToolRegistry::new();
        let paused_nanos = Arc::new(AtomicU64::new(0));
        let tk = WordTokenizer;
        let cancel = Arc::new(AtomicBool::new(false));
        let agent_paused = Arc::new(AtomicBool::new(true)); // пауза до старта
        let mut events = Vec::new();
        let outcome = run_agent_loop(
            &provider,
            &reg,
            vec![ChatMessage::user("q")],
            LoopBounds::default(),
            &budget(),
            &tk,
            &cancel,
            &agent_paused,
            &paused_nanos,
            RunCtx::NONE,
            &mut |e| events.push(e),
        )
        .await;
        assert!(matches!(
            outcome,
            LoopOutcome::BudgetExhausted {
                kind: BudgetKind::Paused,
                ..
            }
        ));
        assert!(
            !events.iter().any(|e| matches!(e, AgentEvent::Error(_))),
            "Paused НЕ эмитит терминальный Error: {events:?}"
        );
    }
}
