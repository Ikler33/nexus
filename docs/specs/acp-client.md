# ACP-CLIENT — драйв ВНЕШНЕГО ACP-агента (Hermes и пр.)

> Spec v1.0 (2026-06-24). Срез ACP-1. Decision-complete, с вложенными фиксами adversarial-критики.
> Зеркальная пара к [`agent-connect.md`](agent-connect.md): там мы — **сервер** нашего протокола для
> встроенного Castor; здесь мы — **клиент** [Agent Client Protocol](https://agentclientprotocol.com)
> (Apache-2.0, Zed) и драйвим ЧУЖОГО агента, спавня его подпроцессом.

## 1. Цель
`mode="acp"` в `ai.connection` → desktop спавнит внешний ACP-агент (`acp_command`, напр. `["hermes","acp"]`)
и драйвит его по ACP вместо встроенного движка. Стриминг, tool-calls и аппрув действий работают как у
Castor — UI **не меняется** (мост ACP↔`AgentStreamEvent` живёт целиком в `AcpBackend`). Default остаётся
embedded — **0 регрессии**; CONN-2 `mode=local` тоже не тронут.

## 2. Решения (made)

### 2.1 Крейт `agent-client-protocol` vs хэндролл — **хэндролл (+0 deps)**
Официальный крейт отвергнут: 0.15.x тащит небайтовый рантайм (`futures-io`/`smol`-стиль, ~12 транзитивных
крейтов) несовместимый с нашим `tokio`-framing; 0.14.0 требует futures↔tokio compat-мост. ACP на проводе =
**JSON-RPC 2.0, line-delimited (`\n`), без embedded-newline** — БАЙТ-в-байт наш AF_UNIX-framing (CONN-2).
Поэтому переиспользуем `RpcMessage`/`Transport`/framing и сериализуем ACP-структуры в `params`. Schema-дрейф
ловит пиннированный контракт-тест (`acp_schema_roundtrips_real_payloads`) + независимый мок-агент (§7).

### 2.2 Архитектура
```
AcpBackend (apps/desktop, AgentBackend)         crates/nexus-core/src/agent/connect/
  run() ─ spawn StdioTransport ───────────────▶  stdio.rs  (Command piped, kill_on_drop, drain stderr)
        ─ AcpClient::new(transport) ──────────▶  acp/client.rs  (bidirectional read-loop)
        ─ initialize / session/new (30s) ────▶  acp/schema.rs  (v1 serde wire-типы)
        ─ drive_run task: select! {
            session/prompt (NO timeout),
            updates.recv  → map_update → Channel,
            perms.recv    → handle_permission → Proposal → блок до agent_approve → respond
          }
```
- **`StdioTransport::spawn`**: пайпит stdin/stdout/stderr, `kill_on_drop(true)` (нет осиротевших процессов),
  ДРЕНИРУЕТ stderr в `tracing` (иначе piped-буфер дедлокнет агента). Framing общий (`connect/framing.rs`).
- **`AcpClient`** (двунаправлен — в отличие от half-duplex `ConnectClient`): read-loop разводит `Response`→
  ждущим по `id`, `session/update`→updates-канал, входящий `session/request_permission`→perms-канал;
  `fs/*`/`terminal/*` агента → `method_not_found` (мы объявили `capabilities=false` — агент их звать не должен;
  если зовёт — **fail-closed без зависа**). Закрытие транспорта → провал всех ждущих.
- **`AcpBackend`**: соединение-на-прогон (один процесс агента на `run`; reuse/мультитёрн — отложено, см. R4).

### 2.3 Мост аппрува (ядро среза)
Hermes пишет файлы **в своей песочнице сам** (`capabilities.fs=false`) — наш «актуатор» = ТОЛЬКО решение.
Входящий ACP `session/request_permission` → `AcpBackend`:
1. чеканит синтетический `action_id` (`AtomicI64`), запоминает `(rpc_id, options)` в `pending_perms`;
2. эмитит `AgentStreamEvent::Proposal{files:[AgentProposedFile{path,add,del,status,action_id}]}` —
   ровно тот же DTO, что у Castor → фронт (`agent_approve`/`UiDecisionSource`/стор) **не трогаем**;
3. блокирует, пока юзер не решит; `agent_approve(action_id, approve)` → `pick_outcome` выбирает
   ACP-`optionId` по виду опции и отвечает `Response{outcome:{selected,optionId}|cancelled}`.

Трансляция `optionId`↔`action_id` живёт ЦЕЛИКОМ в `AcpBackend`. `pick_outcome` — чистая функция,
**fail-closed**: approve без `allow_once`/`allow_always` опции → `cancelled` (НИКОГДА не авто-allow);
reject → `reject_once`/`reject_always`; нестандартный набор → `cancelled`.

### 2.4 Маппинг `session/update` → `AgentStreamEvent`
| ACP | наше |
|---|---|
| `agent_message_chunk` / `agent_thought_chunk` | копится в `answer` → финальный `Final{text}` (accum в drive-цикле) |
| `tool_call` | `ToolCall{id,kind,args}` (`kind` через `acp_kind_to_display`) |
| `tool_call_update`(completed/failed) | `ToolResult{id,content,is_error}` (failed→`is_error=true`) |
| `tool_call_update`(pending/in_progress) | — (не финализируем tool) |
| `session/request_permission` | `Proposal` (см. §2.3) |
| `stopReason` end_turn/… | `Final{text:answer}` · cancelled/refusal-семантика честная |
| transport closed mid-run | висящие permission → `cancelled`, затем `Error` |

### 2.5 Конфиг
`ai.connection`: `mode:"acp"` + `acp_command:Vec<String>` (program+args) + `acp_cwd:Option<String>`
(дефолт = vault root). Ключи на диске — **snake_case** (как весь `ai.*`; `ConnectionConfig` без
`rename_all`). Толерантные десериализаторы (`de_tolerant_string_vec`/`_opt_string`): мусорное значение →
поле `None`, но **НИКОГДА** не роняет `ai.chat`/`ai.embedding` (defense против data-loss).

## 3. Управляющие методы
- `run` → spawn → `initialize`(30s) → `session/new`(30s) → drive-task → `run_id`. R4-гард: один активный прогон.
- `cancel` → `session/cancel` notification + отмена ждущих RPC.
- `pause`/`resume` → `Err` (ACP v1 без server-side pause — см. R3; честно, не молчаливый no-op).
- `undo` → `Ok(0)` (агент пишет в своей песочнице — нашего леджера для его записей нет, R3).
- `approve` → `pick_outcome` → `respond` (§2.3).

## 4. Граница и инварианты безопасности
- `capabilities={fs:false, terminal:false}` — агент не делегирует нам запись/терминал; если зовёт —
  `method_not_found` (fail-closed).
- Аппрув fail-closed по умолчанию (§2.3); перегруз perms-канала (закрыт consumer) → `cancelled` сам.
- `kill_on_drop` — дроп бэкенда убивает подпроцесс. stderr дренируется (анти-дедлок + наблюдаемость).
- Embedded/CONN-2 пути **байт-идентичны** (ветка `ConnectionMode::Acp` добавлена, существующие не тронуты).

## 5. Тестирование
- **`acp/schema.rs`**: пиннированные реальные ACP-payload'ы round-trip (дрейф схемы падает громко).
- **`acp/client.rs`**: полный bidirectional round-trip через in-process `ChannelTransport` (скриптованный
  мок-агент: initialize→session/new→prompt+стрим+permission→end_turn) + closed-transport→`Err` (не зависает).
- **`stdio.rs`** (unix): `cat`-roundtrip, `true`→EOF→`None`, отсутствующий бинарь→`Err`.
- **`agent_backend.rs` (acp_backend)**: чистые функции — `extract_proposal` (diff→New/+add / old_text→Edit /
  no-diff→degraded), `map_update` (tool_call / completed→ok / failed→is_error / pending→пусто), `chunk_text`,
  `pick_outcome` (allow/reject/**fail-closed cancelled**).
- **`tests/acp_e2e.rs`**: E2E против РЕАЛЬНОГО подпроцесса — спавнит `examples/mock_acp_agent` (независимая
  реализация контракта) через `StdioTransport`, драйвит `AcpClient`'ом весь путь до `end_turn`. Покрывает
  единственный путь, не покрытый юнитами: настоящий процесс + pipe-framing + bidirectional loop вместе.

## 6. Ограничения первого среза (R1–R6, честно)
- **R1 — без истории/автономии-переноса**: `session/prompt` = один ход; мультитёрн-история и перенос нашей
  autonomy-posture в чужого агента не делаются (его автономия — его дело).
- **R2 — владение vault**: ACP-агент работает в СВОЁМ cwd/песочнице и пишет файлы сам; за тем, что
  `acp_cwd` указывает на тот же vault, следит пользователь (мы туда не пишем за него).
- **R3 — без undo/без pause**: `undo→Ok(0)`, `pause/resume→Err` (ACP v1 их не несёт; см. §3).
- **R4 — один прогон на соединение**: соединение-на-прогон, параллельные прогоны на одном процессе не
  поддержаны (R4-гард); reuse процесса между прогонами — отложено.
- **R5 — без reconnect**: падение/закрытие подпроцесса обрывает прогон (`Error`); авто-переподключения нет.
- **R6 — session/fork (unstable) выключен**: форк/resume сессии ACP помечены нестабильными — не используем.

## 7. Мок-агент для CI
`crates/nexus-core/examples/mock_acp_agent.rs` — **независимая** (не делит код с клиентом) реализация
ACP-контракта на синхронном std-IO: проходит happy-path и служит оракулом дрейфа схемы
([`feedback_mock_must_match_backend`]). Запускается из `tests/acp_e2e.rs` (досборка on-demand через
`env!("CARGO")` делает тест устойчивым к `--test`-фильтру; +0 deps).
