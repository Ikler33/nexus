# ACP-SERVER — хостинг Castor для ВНЕШНЕГО ACP-клиента (`nexus acp`)

> Spec v1.0 (2026-06-24). Срез ACP-2. Decision-complete.
> Зеркальная пара к [`acp-client.md`](acp-client.md): там мы — **клиент** [Agent Client Protocol](https://agentclientprotocol.com)
> (Apache-2.0, Zed) и драйвим ЧУЖОГО агента; здесь мы — **сервер**: хостим прогон Castor поверх stdio,
> а внешний ACP-клиент (Zed/JetBrains или наш `AcpClient`) драйвит НАС.

## 1. Цель
Подкоманда `nexus acp --vault <P> [--actuator] [--auto]` (в бинаре `nexus-cli`, НЕ в `nexus-agentd`)
поднимает ACP-СЕРВЕР по stdin/stdout. Внешний ACP-клиент спавнит её подпроцессом и говорит
line-delimited JSON-RPC 2.0 (ACP v1): `initialize` → `session/new` → `session/prompt` → … Стриминг,
tool-calls и аппрув действий едут по протоколу. Default — **SAFE**: актуатор OFF, автономия confirm,
permission fail-closed. **0 регрессии** для `nexus agent`/agentd.

## 2. Решения (made)

### 2.1 Бинарь: `nexus-cli`, не `nexus-agentd` — **CLI**
ACP — «клиент-спавнит-агента-по-stdio» (см. [`stdio.rs`](../../crates/nexus-core/src/agent/connect/stdio.rs)).
Per-invocation CLI-процесс, завершающийся на EOF stdin, — верный жизненный цикл. `nexus-agentd` —
долгоживущий AF_UNIX-демон, говорящий НАШ протокол (`agent/run`/`agent/approve`); навесить на него ещё
stdio-ACP-петлю = мультиплекс его уже занятого stdio + новая внешняя поверхность атаки внутри демона,
который держит живой prod-сокет. `nexus-cli/src/agent.rs` уже несёт ровно ту композицию, что нужна
ACP-серверу (`build_deps`/`load_local_config`/run-lifecycle) → переиспользуем (promote `pub(crate)`).

### 2.2 Хэндролл (+0 deps), façade над `run_agent_session`
Как у ACP-1: крейт `agent-client-protocol` отвергнут (несовместимый с tokio рантайм). ACP на проводе =
JSON-RPC 2.0 line-delimited — БАЙТ-в-байт наш framing. Переиспользуем `RpcMessage`/`Transport`/`framing`/
ACP wire-типы (`schema.rs`)/`ACP_PROTOCOL_VERSION`/`run_agent_session`+`SessionSpec`+`AgentEventForwarder`/
`DecisionSource`+`ProposalBatch`+`BatchDecision`/`run_store`/`LoopOutcome`/`ChatMessage`/`AgentEvent`.
`serve_acp(transport, cfg)` — façade: знает только `Arc<dyn Transport>` (юниты — `channel_pair`; CLI —
`StdinStdoutTransport`).

### 2.3 Outbound через `serde_json::json!` (НЕ Serialize)
serde `#[serde(other)]` + `Serialize` НЕВОЗМОЖНО (serde запрещает сериализовать `other`-арм) —
`SessionUpdate`/`ToolCallContent`/`ContentBlock`/`ToolKind`/`AcpPlan*` все его несут. Поэтому ВЕСЬ
server→client outbound (session/update, request_permission, ответы) строится через `json!` в `server.rs`
(зеркало `mock_acp_agent.rs`). В `schema.rs` лишь добавлен `Deserialize` к 3 входящим param-типам
(NewSessionParams/PromptParams/CancelParams). Формы JSON пиннятся юнит-тестами против тех же payload'ов,
что парсит `AcpClient`.

### 2.4 Read-loop НЕ блокируется (out-of-order keystone)
`session/prompt` идёт в СПАВНЕННУЮ drive-задачу, которая САМА отвечает на prompt-id ПОСЛЕ
стрима+permission. Так `session/cancel` и client-`Response` (ответы на наши permission-запросы) текут
конкурентно (зеркало `acp_read_loop`). `Transport::send` сериализован (StdinStdoutTransport лочит Stdout;
`send_frame` пишет одну целую строку атомарно). Контракт «один consumer recv» соблюдён (recv зовёт лишь loop).

## 3. Методы

| метод | обработка |
|---|---|
| `initialize` | params не object → invalid_params; иначе `Ok({protocolVersion: 1})` НЕЗАВИСИМО от запрошенной версии (ACP-конвенция: объявляем свою). fs/terminal-caps клиента игнорим (не зовём fs/*, terminal/*). |
| `session/new` | парс `{cwd, mcpServers}`. Уже есть сессия → invalid_params (R1). `cwd` логируется, vault НЕ репойнтит (R7); `mcpServers` игнор (лог). `Ok({sessionId})`. |
| `session/prompt` | парс `{sessionId, prompt[]}`. Не та сессия / нет сессии → invalid_params. CAS active false→true (R2: второй активный → invalid_params), сброс cancel. Текст из Text-блоков (Other/image/audio игнор); пусто/>256KiB → invalid_params. НЕ отвечаем сразу: drive-задача (create_run → run_agent_session → finish_run) ответит `{stopReason}` ПОСЛЕ стрима+permission. Без server-таймаута (cold-start 1-3мин). |
| `session/cancel` | request ИЛИ notification: взвести cancel сессии + провалить ждущие permission → Cancelled. request → `Ok({})`; notification → без ответа. |
| прочее (`fs/*`, `terminal/*`, `session/load`, `session/fork`, `session/set_mode`, …) | method_not_found (-32601). Никогда не висим. |
| client-`Response` | роутинг в `perm_pending[id]` (ответ на наш `session/request_permission`). Неизвестный id → дроп. |

Битые/oversize params → invalid_params (-32602). EOF (родитель закрыл stdin): провалить ВСЕ ждущие
permission с Err (decide() → reject_all), взвести cancel, вернуться — без зависов.

## 4. Маппинг события цикла → `session/update` (`map_event_to_acp`, чистая, ИСЧЕРПЫВАЮЩАЯ + `_`)

| `AgentEvent` | ACP `session/update` (params = {sessionId, …}) |
|---|---|
| `AssistantToken(s)` | `agent_message_chunk` `{content:{type:text, text:s}}` |
| `ToolCall{id,kind,args}` | `tool_call` `{toolCallId:id, title:"<kind clip(args)>", kind:<acp_tool_kind→edit/read/search/other>, status:in_progress}` |
| `ToolResult{id,content,is_error}` | `tool_call_update` `{toolCallId:id, status:is_error?failed:completed, content:[{type:content, content:{type:text, text:clip(content)}}]}` |
| `PlanProposed{steps}` | `plan` `{entries:[{content:label, priority:medium, status:pending/in_progress/completed; Failed→completed}]}` (full-list, ACP-1b) |
| `Error(s)` | `agent_message_chunk` `{text:"[error] "+s}` (sanitized — без путей); stopReason на Response |
| `Proposal`/`Diff` | `[]` — permission-поверхность через `request_permission` (decide()), не дублируем |
| `Final` | `[]` — текст уже стримился; Final → Response stopReason `end_turn` |
| `ContextUsage`/`PlanStepStatus`/`ExecProposal`/`ExecResult`/`SubagentStatus`/`Report` | `[]` (нет ACP-эквивалента / exec/делегирование/research выключены, slice-1) |
| `_` (non_exhaustive) | `[]` |

stopReason (`stopreason_from_outcome`, на Response): Final→`end_turn`; Cancelled/Paused→`cancelled`;
Tokens/Steps/WallClock→`max_turn_requests`; Error→`refusal` (текст уже застримлен chunk'ом, валидный
stopReason проще для IDE, чем JSON-RPC-ошибка).

## 5. DecisionSource (`session/request_permission`) — ЕДИНСТВЕННОЕ место авторизации записи
`AcpServerDecisionSource` реализует `actuator::DecisionSource` (drop-in; гейт/`run_agent_session` не
тронуты). Инверсия ACP-1: ТАМ клиент получает request_permission, ЗДЕСЬ мы его шлём и ждём ответ.
`decide(batch)`:
1. id = `next_perm_id.fetch_add` (старт `1_000_000_000` — НИКОГДА не пересечётся с client-id, тот с 1;
   belt-and-suspenders, направления и так раздельны).
2. params (`proposal_to_permission_params`): `toolCallId="run{run_id}-perm{id}"`, `title="N change(s): +A/-D"`,
   `kind:edit`, `content:[{type:diff, path:item.target_rel, newText:""}]`, options `[allow, reject]`.
   ProposalItem несёт лишь path+add/del → **деградированный diff** (R4): клиент видит КАКИЕ файлы + риск +
   счётчики, не литеральный контент. params строятся из СОБСТВЕННОГО `batch` decide() → консистентны (нет гонки).
3. register oneshot в `perm_pending[id]`; `transport.send(request)`. send-fail → remove + reject_all.
4. await oneshot с таймаутом 5 мин (cancel дополнительно проваливает карту через `session/cancel`). На
   timeout/oneshot-closed(EOF)/Cancelled → reject_all.
5. `outcome_to_batch_decision`: ТОЛЬКО `selected`+`optionId==allow` → `from_pairs(Approve для ВСЕХ айтемов)`
   (пер-батч: один Allow = весь батч); reject/неизвестная опция/cancelled/parse-miss/Err → reject_all.

**Пер-батч, не пер-файл** (R4): ACP `request_permission` = один toolCall с одним набором options = один
outcome; пер-файловой ACP-семантики нет. Гейт эмитит один ProposalBatch на changeset, а `BatchDecision`
рубеж-2 (айтем без явного Approve = Reject) => один Allow одобряет РОВНО перечисленные айтемы.

## 6. Граница и инварианты безопасности (SAFE BY DEFAULT, fail-closed)
- **Актуатор OFF по умолчанию**: без `--actuator` → `run_agent_session` ставит ТОЛЬКО стабы (echo/noop) —
  write-инструментов в реестре НЕТ → vault не пишется, decide() НЕ зовётся. Единственный эффект дефолтного
  `nexus acp` — строка `agent_runs` (как у `nexus agent`).
- **Автономия confirm по умолчанию**: каждый Confirm-тир (и Auto за blast-cap) → `request_permission`
  (fail-closed: нет явного allow → reject_all). `--auto` авто-применяет ЛИШЬ Auto-тир; Confirm-тир (риск)
  всё равно ждёт явного разрешения клиента. Даже под `--auto` нельзя записать рисковую правку без allow.
- **HardBlocked никогда не аппрувится**: classify режет HardBlocked (запись вне canon_root, skill.save при
  выключенном learning) ДО decide() → не становится permission-запросом.
- **canon_root канонизирован** (`resolve_vault`); гейт держит ВСЕ записи под ним; client-`cwd` ИГНОРИРУЕТСЯ
  (нельзя репойнтить vault — R7).
- **НЕТ `--yes`/ApproveAll по ACP**: внешний клиент — единственный аппрувер; авто-одобрение по протоколу
  побило бы гейт (намеренно опущено).
- **stdout — ИСКЛЮЧИТЕЛЬНО канал протокола**: любой `println!` испортит JSON-RPC → всё логирование/баннеры
  → stderr/tracing. `StdinStdoutTransport` владеет stdout единолично.
- **Нет сети ОТ ACP-сервера**: говорит лишь по своим stdin/stdout (пайп клиента). LLM-провайдер — тот же
  EgressPolicy-allowlist из local.json, что у `nexus agent` (audit в БД через build_deps).
- T8/peer-uid N/A (stdio: родитель, спавнивший нас, уже владеет нашими пайпами — граница доверия = spawn).

## 7. R-лимиты (честно)
- **R1** — ОДНА сессия на соединение: вторая `session/new` → invalid_params. Мультиплекс на одном stdio не
  поддержан; один клиент = один процесс `nexus acp` = одна сессия. Параллелизм — параллельными процессами.
- **R2** — мультитёрн, ОДИН активный ход: `session/prompt` можно звать повторно (аккумулируемая in-memory
  история, cap 16 сообщений, только успешные ходы), но второй prompt при активном → invalid_params.
- **R3** — агент пишет vault НАПРЯМУЮ через НАШ гейт, не через ACP-fs-callbacks: caps fs/terminal=false, мы
  не зовём fs/*, terminal/*. Запись — через dispatch_action под canon_root (blast-cap + undo + audit) лишь
  после явного allow.
- **R4** — пер-батч permission, деградированный diff (path+add/del в title, пустой newText); полный контент
  по ACP в slice-1 НЕ шлётся (ProposalItem без нового текста). Пер-файл + богатый diff — отложено.
- **R5** — нет undo/pause как ACP-методов: ACP v1 несёт только `session/cancel` (кооп-отмена + fail-close
  ждущих perms). Undo применённого — вне ACP (`nexus agent /undo` / `agentd agent/undo` на тех же строках).
- **R6** — нет session/load, session/fork, session-resume (ACP unstable), нет reconnect: закрытие stdin
  терминирует процесс; ждущие permission → Reject; ход отменён. Спавнящий клиент рестартит нас. Нет
  session/delete в v1 (освобождается на выходе).
- **R7** — один фиксированный vault: задан `--vault` (или NEXUS_VAULT) при спавне; `session/new.cwd` и
  `mcpServers` логируются/игнорируются. Клиент не репойнтит vault и не цепляет MCP.
- **R8** — нет model_override: сервер использует один сконфигурированный провайдер из ai.chat; ACP-prompt
  не несёт honored-поля модели.
- **R9** — DoS-капы: prompt ≤ 256 KiB (oversize → invalid_params); кадр ≤ 1 MiB + закрытие после 64 подряд
  malformed (из framing.rs); ограниченный канал событий (try_send, дроп при переполнении); permission
  decide() таймаут 5 мин → fail-closed; unknown method → method_not_found; неизвестные notification/
  ContentBlock-варианты игнорятся (forward-compat).

## 8. Тестирование
- **`schema.rs`**: пиннированные ACP-payload'ы round-trip + входящий парс NewSessionParams/PromptParams/
  CancelParams (инверсия — сервер их читает).
- **`server.rs`** (юниты по `channel_pair` + локальный FakeProvider, без LLM, без подпроцесса): initialize/
  session-new/prompt-стрим-end_turn/permission(allow/reject/cancelled/unknown-option/transport-close →
  fail-closed write/no-write)/unknown-method/malformed/second-session(R1)/second-prompt-active(R2)/
  multi-turn-history/cancel/auto-tier-без-permission/eof-clean/oversized + чистые фн (map_event/proposal-
  params/outcome→decision/stopreason).
- **`acp.rs`** (CLI): разбор флагов + дефолтная поза (без --actuator → actuator OFF; без --auto → confirm).
- **`tests/acp_server_e2e.rs`** (unix): наш `AcpClient` драйвит РЕАЛЬНЫЙ `nexus acp` подпроцесс через
  `StdioTransport` — model-free (initialize→session/new→unknown(-32601)→cancel→drop→чистый выход).
  Полный prompt-драйв покрыт in-process юнитами server.rs → e2e остаётся model-free/зелёным.

## 9. Угрозы (threat model, кратко)
- **Stdout pollution** → коррапт JSON-RPC. Митигация: StdinStdoutTransport владеет stdout; всё логирование
  в stderr; e2e падает сразу на загрязнении.
- **Permission deadlock** (decide() ждёт, loop не доставляет Response). Митигация: decide() шлёт через
  транспорт и ждёт oneshot, фаерящийся ОТДЕЛЬНЫМ read-loop'ом через общий perm_pending (как ACP-1). Тесты
  permission_allow/transport_close.
- **EOF мид-permission** → завис drive. Митигация: на EOF loop дренирует perm_pending с Err → decide()
  reject_all; плюс 5-мин таймаут-бэкстоп. Тест permission_transport_close_rejects.
- **id-коллизия** наших permission-id с client-id. Митигация: выделенный счётчик с базы 1_000_000_000.
- **Мультитёрн-cancel травит следующий ход**. Митигация: сброс cancel в начале каждого prompt.

## 10. Scope deferrals (slice-1)
Remote/WS/AF_UNIX ACP-сервер · mcp_servers · model_override · session/load/fork/resume · fs/terminal
client-callbacks · memory recall · skills + skill.save · web-инструменты · delegation/subagents +
deep-research · полный diff-контент по ACP · пер-файловый permission · undo/pause как ACP-методы ·
мульти-сессия на одном соединении · ApproveAll/--yes по ACP. Все — задокументированы выше (R1–R9).
