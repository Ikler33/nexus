import type { AgentFlagsDto, AiConfigDto, AiEndpoint, SetAiResult } from '../tauri-api';

/**
 * Мок раздела настроек «AI / Модели» для браузерного превью / vitest (вне Tauri).
 * Хранит конфиг в памяти; реальная логика (запись `.nexus/local.json` + hot-apply chat +
 * проверка связи) — в Rust `commands/settings.rs`. Здесь — happy-path для UI/тестов.
 *
 * mock-must-match-backend: зеркалит контракт Rust-команд, включая agent-флаги. `shellSupported=false`
 * — честно для браузер-превью (Linux-песочница недоступна), как и на macOS-десктопе.
 */

let config: AiConfigDto = {
  chat: null,
  embedding: null,
  fast: null,
  agentAutonomy: null,
  sandboxEnabled: false,
  shellEnable: false,
  webAllowPublicFetch: false,
  shellSupported: false,
};

export async function getAiConfig(): Promise<AiConfigDto> {
  return { ...config };
}

export async function setAiConfig(
  chat: AiEndpoint | null,
  embedding: AiEndpoint | null,
  fast: AiEndpoint | null = null,
): Promise<SetAiResult> {
  const embeddingChanged = JSON.stringify(config.embedding) !== JSON.stringify(embedding);
  // Сохраняем agent-флаги (set_ai_config их не трогает — отдельная команда set_agent_flags).
  config = { ...config, chat, embedding, fast };
  return { chatApplied: true, embeddingChanged };
}

/**
 * Зеркало Rust `set_agent_flags`: персист + нормализация. autonomy: невалид → null (= confirm).
 * Когерентность shell↔sandbox: shell без sandbox → false (как apply_agent_flags на trust-boundary).
 */
export async function setAgentFlags(flags: AgentFlagsDto): Promise<AgentFlagsDto> {
  const agentAutonomy =
    flags.agentAutonomy === 'confirm' || flags.agentAutonomy === 'auto'
      ? flags.agentAutonomy
      : null;
  const normalized: AgentFlagsDto = {
    ...flags,
    agentAutonomy,
    shellEnable: flags.shellEnable && flags.sandboxEnabled,
  };
  config = { ...config, ...normalized };
  return normalized;
}

export async function testConnection(url: string): Promise<void> {
  if (!/^https?:\/\/.+/.test(url.trim())) throw new Error('некорректный URL (ожидается http(s)://…)');
  // В превью любой синтаксически верный URL считаем достижимым.
}
