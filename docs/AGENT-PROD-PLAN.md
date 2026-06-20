# Agent-Prod Plan — Nexus агент как сервис (план разработки)

> Источник: мультиагентный анализ конкурентов (hermes-agent, odysseus) + аудит полноты + Stage-2 design-спеки (2026-06-20). Детальные артефакты: `~/Documents/Claude/nexus-agent-design/{PROD-SYNTHESIS,PRODUCT-COMPLETENESS,stage2-specs-raw}.{md,json}`. Этот файл — **источник истины в репо** (CHANGELOG/BACKLOG ссылаются сюда).

## 0. Цель и решение по архитектуре

**Базовое приложение (Qasr) + ИИ-помощник** — самоценный продукт на устройстве, говорит с LLM напрямую. **Агент — ОТДЕЛЬНЫЙ сервис** (`nexus-agentd`, нативный Rust), приложение подключается к нему через **коннектор**. Агент НЕ sandboxed-плагин (это привилегированный peer, не урезанное расширение); плагином/опц.-модулем делаем именно коннектор.

### Трёхслойная модель
| Слой | Что | Где живёт |
|---|---|---|
| **Base** | Qasr-app + ИИ-помощник (RAG/inline/inspector/граф) | устройство |
| **Agent** | `nexus-agentd`: bounded tool-loop, память (3 слоя), актуатор (vault, default-OFF), навыки, scheduler | локаль/риг/VPS/облако |
| **Connector** | тонкий клиент app↔agentd по протоколу AGENT-CONNECT (ACP-совместимый), транспорт подключаемый | в приложении (опц.) |

**Vault-мост сервер↔устройство**: git-sync + конфликт-резолвер (уже в проде).

## 1. Что уже готово (фундамент)
- **Фаза 0**: P0-a..e (egress DNS-guard / durable write-before-act audit / Qwen-токенайзер / retry / fence) + CORE-1 (`nexus-core`) + CORE-2 (`nexus-agentd` скелет). ✅
- **Фаза 1 (ядро агента)**: AGENT-1..6 (цикл / прогоны / память / актуатор-vault default-OFF / undo / kill-switch / privacy) + SKILL-1..3 + INFER-CFG + UI-1 (вкладка Агента в desktop). ✅
- **Подтверждено live (2026-06-20)**: риг `192.168.0.31:8080` (Qwen3.6-27B-MTP, llama.cpp, 32K ctx) **отдаёт OpenAI tool-calling** → агент-цикл поедет на текущем железе, V100 не нужен.

## 2. Роадмап (ребейзнут — Фазы 0/1 готовы)

### PROD-v1 — агент как сервис (vault-only, безопасно, БЕЗ новой security-границы)
1. **AGENT-CONNECT spec → код** (блокирующая зависимость) — протокол + транспорт. Спека: `docs/specs/agent-connect.md`.
2. **agentd-демон**: скелет → реальный headless-демон (вынести bounded-loop), AGENT-CONNECT-сервер.
3. **Connector-модуль** в app (транспорт in-process / WS).
4. **`nexus deploy local`** + config-bootstrap + git-sync мост.
5. **THREAT_MODEL.md** (P0-гейт, параллельно) — `docs/THREAT_MODEL.md`.
6. Параллельный трек десктопа: **Release & Auto-Updater** (распространение приложения).

### DEPLOY-2 — авто-деплой везде
Контейнер-образ (= база Фазы-2 песочницы) · `nexus deploy docker|vps` · compose · топологии local-all-in-one / VPS-агент+удалённый-LLM (через amnezia-VPN) / облако+API.

### Порт от конкурентов (после PROD-v1, дескоупнуто под local-first)
P0: skill learning-loop (curator+usage) · session-search FTS5 · субагенты/делегирование. P1: MCP-клиент · cron NL-jobs · backup/restore · observability. Post-1.0/owner-gated: multi-channel-шлюзы · email · deep-research · voice · marketplace.

### 🔒 Owner-gated security (после PROD-v1)
- **Фаза 2 — sandbox**: rootless Podman `--network=none` + GuardedProxy (AF_UNIX) + headless control-plane + MCP-lite.
- **Фаза 3 — host-actuator**: shell/process/git ActionTargets ВНУТРИ песочницы. Гейт: THREAT_MODEL + env-scrubbing.

## 3. 6 P0-дыр до «готового продукта» (что забывали)
1. 🔴 **Авто-апдейтер** (Tauri-updater не включён, нет подписанных релизов).
2. 🔴 **AGENT-CONNECT протокол** (коннектор не специфицирован) → `docs/specs/agent-connect.md`.
3. 🟡 **Подписанные релизы / packaging** (версия хардкод `0.0.0`).
4. 🟡 **THREAT_MODEL.md** → `docs/THREAT_MODEL.md`.
5. 🟡 **Install / first-run** (onboarding неполный) + user-доки (INSTALL/GETTING-STARTED/CONFIG).
6. 🔴 **Container / VPS-деплой** (нет Dockerfile/.service/`nexus deploy`).

## 4. DoD каждого среза (НЕ забываем)
Каждый срез агент-эпика мержится ТОЛЬКО при:
- **Код** + **тесты в том же PR** (no-tails);
- **Тесты по слоям** (TESTING_STRATEGY.md): unit + integration (выделенный `tests/`-крейт для agentd) + где есть UI — vitest;
- **LIVE-тест агента против рига** (`#[ignore]`, runnable вручную/ночью): tool-loop/connect/actuator на `192.168.0.31:8080` (qwen tool-calling) / `:8084` (gemma fast-util) / `:8083` (bge). Актуатор-live → ТОЛЬКО temp-vault + opt-in флаг;
- **coverage-ратчет не ниже** (`coverage-baseline.json`; добавить per-path floors на новые критичные модули: connector/transport/config-loader);
- **traceability AC↔тест** обновлён;
- **adversarial-ревью** диффа (мульти-рецензент) ПЕРЕД мержем;
- **docs обновлены** (CHANGELOG + затронутые спеки).

### Live-тест-слой агента (новое, под 24/7 риг)
- Env-override эндпоинтов (`NEXUS_CHAT_URL` и т.п.) → на риг.
- Сценарии: (1) tool-loop end-to-end (промпт → tool_call → tool_result → final); (2) AGENT-CONNECT round-trip по каждому транспорту; (3) актуатор apply→undo в temp-vault; (4) fail-closed (dropped-approval→reject_all, pause).
- Нюанс egress: model-эндпоинт (риг, LAN/RFC1918) = chat-класс `deny_private=false` (LAN жив); tool/web-эгресс агента = guarded `deny_private=true`. Проверить в live, что агент достаёт свою модель, но web-инструменты идут через guard.

## 5. Тест-инфра — что добавить (расширение TESTING_STRATEGY.md)
- Выделенный **integration-крейт для `nexus-agentd`** (`crates/nexus-agentd/tests/*.rs`, чёрный ящик).
- **Cross-process e2e** коннектора (in-process / WS).
- **Deploy/release smoke** в CI (pre-release эшелон): `nexus deploy local`/`docker` + health-check; `--help`/launch собранных артефактов.
- Опц. **mutation (`cargo-mutants`)** на security-путях (actuator/egress/connector-auth).
- Адаптировать CI-строгость конкурентов: trivy/container-scan (когда Docker), secret-scan, osv (есть cargo-deny), dependency-review.

## 6. Документация — план (репо `docs/`)
v0.1.0-blocking ⭐: GETTING-STARTED · INSTALLATION · CONFIGURATION-REFERENCE. Далее: ARCHITECTURE-SUMMARY (этот §0) · AGENT-SERVICE · PROTOCOL/agent-connect ✅(этот срез) · THREAT_MODEL ✅(этот срез) · DEPLOYMENT-GUIDE · VISION · ROADMAP · CONTRIBUTING · RELEASE-PROCESS · PLUGIN-DEVELOPER/* · PRIVACY.
