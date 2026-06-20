# AGENT-CONNECT — протокол коннектора app ↔ nexus-agentd

> Spec v1.0 (2026-06-20). Блокирующая зависимость PROD-v1. ACP-совместимый (Agent Client Protocol) поверх нашего `AgentEvent`-контракта. Decision-complete, с вложенными фиксами adversarial-критики Stage-2.

## 1. Цель
Двунаправленный протокол: клиент (desktop / SDK) ↔ сервис (`nexus-agentd`). Маппит наши команды/события агента на ACP-семантику. UI-1-контракт desktop НЕ ломаем (адаптер между `AgentEvent` и протоколом).

## 2. Решения (made)
1. **ACP-семантика как основа** (не изобретаем): message/thought/tool-call/usage chunks, session fork/resume/list, plan-update. Reference: hermes `acp_adapter/server.py`.
2. **Маппинг через адаптер**: `AgentEvent` (AssistantToken/ToolCall/ToolResult/ContextUsage/Proposal/Diff/Final/Error, `#[non_exhaustive]`) ↔ ACP chunks. Desktop UI-1b без изменений.
3. **Транспорт подключаемый** (трейт): **in-process** `tokio::mpsc`/`Arc<Channel>` (default, embedded) · **AF_UNIX-сокет** (локальный не-embedded; OS-права для изоляции юзеров — критика: предпочесть Unix-сокет TCP-loopback'у) · **WS+TLS** (remote, opt-in) · **stdio** (тесты).
4. **Framing = JSON-RPC 2.0**: requests (с `id`) + notifications (без `id`). **id-мультиплексинг out-of-order-safe** (коррелировать по `id`, не FIFO) — несколько прогонов на сессию.
5. **Сессии**: `initialize` → session_id; `session/new|resume|list|fork`. Сессия хранит run-историю, vault-контекст, выбор модели.
6. **Approval двунаправленно**: agentd на Confirm-гейте шлёт `Proposal`-событие; клиент рендерит UI; юзер → `agent/approve` notification (содержит **session_id + run_id + decisions**). Гейт `decide()` ждёт. **Decision-timeout** (critique): default 300s, `AGENT_DECISION_TIMEOUT_SECS`, по таймауту → reject_all (fail-closed).
7. **Pause/kill-switch**: server-side atomic-флаг, проверяется per-step (loop-head / per-tool / per-batch-item). **`agent/control{pause}` пишет в `agent.json` атомарно** (critique: переживает рестарт/дисконнект), load на старте.
8. **Undo**: `agent/undo{session_id, run_id}` → walk `agent_actions` ledger по run_id, state=executed→restore→state=undone. **Идемпотентно** (повтор = no-op). Ответ = число восстановленных.
9. **Version-negotiation**: `initialize{supported_versions:["1.0",…]}` → сервер выбирает наибольший совместимый MINOR; на несовместимость — типизированная ошибка.
10. **Plan-viz**: ACP `plan/update` (entries: content/priority/status) — для граф-визуализации плана (AGENT-2 будет эмитить; пока — из tool-последовательности).
11. **Auth (remote)**: bearer-token, scope = `session_id`+`vault_path`-hash (анти-fixation), хранение keychain(desktop)/env(systemd). Local AF_UNIX — OS-права, без токена.
12. **Optional-поля**: омитим (`#[serde(skip_serializing_if="Option::is_none")]`), не шлём null.
13. **Sanitization ошибок**: в ответ клиенту — generic (`{code, message}`), без путей/токенов; детали — в server-лог.

## 3. Сообщения (каталог v1.0)
**Requests (client→agentd, есть `id`):** `initialize` · `session/new|resume|list|fork` · `agent/run{session_id, prompt, model_override?}` · `agent/undo{session_id, run_id}` · `agent/cancel{session_id, run_id}`.
**Notifications (client→agentd, без `id`):** `agent/approve{session_id, run_id, decisions:[ItemDecision]}` · `agent/control{session_id, pause:bool}`.
**Server→client (stream, notifications):** `agent/event` (обёртка над `AgentEvent`→ACP chunk: assistantToken/toolCall/toolResult/contextUsage/proposal/diff/final/error) · `plan/update`.

## 4. Маппинг `AgentEvent` ↔ ACP (TOOL_KIND_MAP)
Статическая таблица tool→ACP-kind: `note.create→write`, `note.edit→write`, `set_frontmatter→write`, `read_note→read`, `search→search`, … (default `other`). Обе стороны used; покрыта differential-тестом.

## 5. Rust-интерфейсы (скелет P0a)
```rust
// crates/nexus-core/src/agent/connect/mod.rs (новый модуль)
#[async_trait] pub trait Transport: Send + Sync {
    async fn send(&self, msg: RpcMessage) -> ConnectResult<()>;      // server→client / client→server
    async fn recv(&self) -> ConnectResult<Option<RpcMessage>>;        // None = closed
}
pub struct ChannelTransport { /* tokio::mpsc pair */ }                 // P0a, in-process

pub enum RpcMessage { Request{id, method, params}, Notification{method, params}, Response{id, result|error} }

// agentd-сторона: диспетчер метод→хендлер
pub struct ConnectServer { /* RunRegistry, sessions, AgentMemory, ... */ }
impl ConnectServer { pub async fn serve(self, t: impl Transport); }   // цикл recv→dispatch

// connector-клиент (desktop/SDK)
pub struct ConnectClient { /* транспорт + correlation-map по id */ }
```
Desktop: текущие tauri-команды (`agent_run/approve/...`) → `ConnectClient` поверх `ChannelTransport` (P0d, механически; UI без изменений). **Breaking (UI-1a):** `agent_approve` получает `session_id` (critique-fix).

## 6. Security
- Local-first default: in-process / AF_UNIX (mode 0600). Remote = WS+TLS + token, opt-in.
- Approval fail-closed (dropped/timeout → reject_all). Pause durable (agent.json). Undo идемпотентен.
- Ошибки sanitized. Token per-session scope, keychain/env (не plaintext-конфиг).
- Egress: коннектор НЕ открывает сырые сокеты к LLM; model-эгресс — через существующий `GuardedClient`/INFER-CFG.

## 7. Rollout (срезы)
- **P0a** [L]: `Transport`-трейт + `ChannelTransport` + JSON-RPC диспетчер + `initialize`/`agent/run`/`agent/approve`/`agent/control`/`agent/undo` + version-negotiate + error-map. **Тесты:** unit (dispatch/mapping/version/error) + **live** (tool-loop через ChannelTransport на риг 192.168.0.31:8080).
- **P0b** [M]: session-таблица (миграция) + EventSink-адаптер (`AgentEvent`→ACP chunk).
- **P0c** [S]: version/error edge-cases, optional-field omit.
- **P0d** [S]: desktop wire (ConnectClient/ChannelTransport; +session_id в agent_approve).
- **P1a** [M]: WS+TLS (tokio-tungstenite/rustls), bind 127.0.0.1:8776. **P1b** [M]: token-auth. **P1c** [S]: systemd-unit + docker-compose шаблон.

## 8. Тест-план (DoD)
- **unit**: RPC-dispatch, AgentEvent↔ACP маппинг (differential на TOOL_KIND_MAP), version-negotiate, error-sanitize.
- **integration (cross-process)**: connect round-trip по ChannelTransport (P0a) и WS (P1) — run→stream→approve→apply→undo.
- **fail-closed**: dropped-approval→reject_all, decision-timeout→reject_all, pause-per-step останавливает запись.
- **fuzz**: malformed/out-of-order JSON-RPC framing.
- **LIVE (риг 24/7)**: `#[ignore]` тест — полный tool-loop на `192.168.0.31:8080` (qwen tool-calling), актуатор apply→undo в **temp-vault**, проверка что model-эгресс (LAN) проходит, а web-tool-эгресс идёт через guard.
- coverage-floor на `agent::connect` в `coverage-baseline.json`; traceability AC↔тест; adversarial-ревью перед мержем.
