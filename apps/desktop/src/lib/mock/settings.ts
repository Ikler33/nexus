import type {
  AgentConnectionDto,
  AgentFlagsDto,
  AiConfigDto,
  AiEndpoint,
  SetAiResult,
} from '../tauri-api';

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
  agentActuatorEnabled: false,
  sandboxEnabled: false,
  shellEnable: false,
  webAllowPublicFetch: false,
  skillsLearningEnabled: false,
  agentSkillsDir: null,
  delegationEnabled: false,
  researchEnabled: false,
  connection: {
    mode: 'embedded',
    socket: null,
    acpCommand: null,
    acpCwd: null,
    acpTransport: null,
    acpSshHost: null,
    acpSshKey: null,
    acpRemoteCommand: null,
  },
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

/** CONN-4/ACP-1b/ACP-REMOTE-SSH: зеркало Rust `set_agent_connection` — нормализует mode (мусор →
 *  embedded), хранит сокет/acpCommand/acpCwd + ssh-поля (только при Some; null → не трогаем). Возвращает
 *  записанное (mock-must-match-backend: те же None→keep/empty→clear семантики, что apply_acp). */
export async function setAgentConnection(
  mode: AgentConnectionDto['mode'],
  socket: string | null,
  acpCommand: string | null = null,
  acpCwd: string | null = null,
  acpTransport: string | null = null,
  acpSshHost: string | null = null,
  acpSshKey: string | null = null,
  acpRemoteCommand: string | null = null,
): Promise<AgentConnectionDto> {
  const m: AgentConnectionDto['mode'] =
    mode === 'local'
      ? 'local'
      : mode === 'remote'
        ? 'remote'
        : mode === 'acp'
          ? 'acp'
          : 'embedded';
  // None → keep existing; непустой → trimmed; пустой → null (clear) — как apply_acp на бэке.
  const keep = (val: string | null, cur: string | null) =>
    val === null ? cur : val.trim() || null;
  const next: AgentConnectionDto = {
    mode: m,
    socket: keep(socket, config.connection.socket),
    acpCommand: keep(acpCommand, config.connection.acpCommand),
    acpCwd: keep(acpCwd, config.connection.acpCwd),
    acpTransport: keep(acpTransport, config.connection.acpTransport),
    acpSshHost: keep(acpSshHost, config.connection.acpSshHost),
    acpSshKey: keep(acpSshKey, config.connection.acpSshKey),
    acpRemoteCommand: keep(acpRemoteCommand, config.connection.acpRemoteCommand),
  };
  config = { ...config, connection: next };
  return next;
}

/** CONN-4/ACP-1b: в браузер-превью/vitest ни сокета agentd, ни процесса ACP-агента нет — честно
 *  «недоступен» (mock-must-match-backend: реальная команда тоже вернёт ошибку без демона/агента). */
export async function testAgentConnection(): Promise<string> {
  throw new Error('подключение недоступно (превью)');
}
