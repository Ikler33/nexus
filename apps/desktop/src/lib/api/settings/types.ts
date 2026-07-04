/**
 * DTO-типы settings-домена (F-2d): AI-конфигурация (`ai.*` — chat/embedding/fast + agent-флаги +
 * подключение), результат записи конфига, consent-конфиг web-поиска (W-3). Зеркала Rust-структур
 * (`settings::*` / `WebSearchConfig`) — контракт провода `invoke`. Потребители импортируют по-прежнему
 * из `lib/tauri-api` (barrel-реэкспорт).
 */

// `AgentConnectionDto` (agent-домен, CONN-4 `ai.connection`) — вложенное поле `AiConfigDto.connection`;
// импорт type-only (в рантайме стирается, цикла нет — тот же паттерн, что у `lib/mock/*`).
import type { AgentConnectionDto } from '../agent/types';

/** AI-эндпоинт настроек (зеркалит Rust `settings::EndpointDto`). `model` опционален. */
export interface AiEndpoint {
  url: string;
  model: string | null;
}
/** Текущая AI-конфигурация для формы настроек (зеркалит Rust `settings::AiConfigDto`).
 *  `AgentConnectionDto` (CONN-4 `ai.connection`) живёт в `lib/api/agent/types.ts`. */
export interface AiConfigDto {
  chat: AiEndpoint | null;
  embedding: AiEndpoint | null;
  /** Утилитарная мелкая модель (`ai.fast`) — inline/судья/новости. */
  fast: AiEndpoint | null;
  /** CONN-4 `ai.connection`: режим подключения агента (embedded|local|remote) + сокет для local. */
  connection: AgentConnectionDto;
  // Agent-флаги в `.nexus/local.json`. ПОСЛЕ AGENT-0.2/0.6 десктоп-`agent_run` ЧИТАЕТ часть рантаймом
  // (`agentActuatorEnabled`/`ai.web`/`ai.agent_skills_dir`) — тогглы управляют И десктоп-агентом Castor,
  // И headless `nexus-agentd`. Автономию прогона десктоп берёт per-run из UI. См. AgentFlagsDto.
  /** `ai.agent_autonomy` («confirm»|«auto»): дефолт-постура headless-коннектора. `null` → confirm. */
  agentAutonomy: string | null;
  /** `ai.agent_actuator_enabled`: мастер-свитч РЕАЛЬНЫХ действий агента в vault (default-OFF → заглушки). */
  agentActuatorEnabled: boolean;
  /** `ai.sandbox_enabled`: мастер-свитч OS-песочницы (Linux-only). Предпосылка shell-exec. */
  sandboxEnabled: boolean;
  /** `ai.shell_enable`: host-exec в песочнице (Confirm, никогда Auto). Требует sandbox + Linux. */
  shellEnable: boolean;
  /** `ai.web.allow_public_fetch`: снимает allowlist с агентского `web.fetch` (публичный egress). */
  webAllowPublicFetch: boolean;
  /** W-10 `ai.skills.learning_enabled`: owner-gated самообучение (агент авторствует навыки). */
  skillsLearningEnabled: boolean;
  /** W-10 `ai.agent_skills_dir`: каталог SKILL.md (отн. vault или абсолютный). `null` — навыков нет. */
  agentSkillsDir: string | null;
  /** W-24 `ai.delegation.enabled`: owner-gated делегирование субагентам (default-OFF). */
  delegationEnabled: boolean;
  /** W-25 `ai.research.enabled`: owner-gated deep-research (default-OFF). Требует delegation+web+actuator. */
  researchEnabled: boolean;
  /** Поддержана ли песочница/host-exec на ЭТОЙ платформе (Linux-only) — фронт дизейблит sandbox/shell. */
  shellSupported: boolean;
}

/** Записываемый поднабор agent-флагов (зеркалит Rust `settings::AgentFlagsDto`). */
export interface AgentFlagsDto {
  /** «confirm»|«auto»; иное/`null` → дефолт confirm (ключ не пишется в local.json). */
  agentAutonomy: string | null;
  /** `ai.agent_actuator_enabled`: мастер-свитч реальных vault-действий агента (default-OFF). */
  agentActuatorEnabled: boolean;
  sandboxEnabled: boolean;
  shellEnable: boolean;
  webAllowPublicFetch: boolean;
  /** W-10 `ai.skills.learning_enabled` (owner-gated, default-OFF). */
  skillsLearningEnabled: boolean;
  /** W-10 `ai.agent_skills_dir`: каталог навыков (пусто/`null` → ключ убирается). */
  agentSkillsDir: string | null;
  /** W-24 `ai.delegation.enabled` (owner-gated, default-OFF). */
  delegationEnabled: boolean;
  /** W-25 `ai.research.enabled` (owner-gated, default-OFF). */
  researchEnabled: boolean;
}

/** Результат записи AI-конфига (зеркалит Rust `settings::SetAiResult`). */
export interface SetAiResult {
  /** Chat применён немедленно (без перезапуска). */
  chatApplied: boolean;
  /** Embedding изменился → нужен перезапуск приложения для переиндексации. */
  embeddingChanged: boolean;
}

/** Конфиг web-агента (W-3, зеркалит Rust `WebSearchConfig`): URL SearXNG = consent на эгресс к нему. */
export interface WebSearchConfig {
  enabled: boolean;
  url: string;
}
