import * as mockSettings from '../../mock/settings';
import { bridge } from '../bridge';
import type {
  AgentFlagsDto,
  AiConfigDto,
  AiEndpoint,
  SetAiResult,
  WebSearchConfig,
} from './types';

/**
 * Settings-домен (F-2d): «хвост» настроек `.nexus/local.json`, НЕ ушедший в agent-домен (F-2c) —
 * AI-конфиг (chat/embedding/fast, hot-apply), agent-флаги (агентд-only) и consent-конфиг web-поиска
 * (W-3, SearXNG). Все вызовы — через `bridge` (Tauri ↔ мок `lib/mock/settings`); потребители ходят сюда
 * по-прежнему через `tauriApi.settings`/`tauriApi.websearch` (barrel-реэкспорт в `lib/tauri-api.ts`).
 * Подключение агента (CONN-4/ACP) живёт в agent-домене (`agentConnection`), реэкспорт под прежними
 * именами `settings.setAgentConnection`/`testAgentConnection` — в барреле.
 */
export const settings = {
  /** Текущая AI-конфигурация из `.nexus/local.json` — для префилла формы (раздел «AI / Модели»). */
  getAiConfig: (): Promise<AiConfigDto> =>
    bridge<AiConfigDto>('get_ai_config', undefined, () => mockSettings.getAiConfig()),

  /**
   * Записывает AI-конфиг в `.nexus/local.json` (сохраняя прочие ключи) и ГОРЯЧО применяет chat.
   * `embeddingChanged` в ответе → UI просит перезапуск (индексатор перечитает конфиг при старте).
   */
  setAiConfig: (
    chat: AiEndpoint | null,
    embedding: AiEndpoint | null,
    fast: AiEndpoint | null = null,
  ): Promise<SetAiResult> =>
    bridge<SetAiResult>('set_ai_config', { chat, embedding, fast }, () =>
      mockSettings.setAiConfig(chat, embedding, fast),
    ),

  /** Проверка связи с LLM-эндпоинтом (пробный GET `/v1/models`). Резолвится = достижим; throw = нет. */
  testConnection: (url: string): Promise<void> =>
    bridge<void>('test_ai_connection', { url }, () => mockSettings.testConnection(url)),

  /**
   * Персистит agent-флаги (агентд-only) в `.nexus/local.json`. В ОТЛИЧИЕ от setAiConfig — без
   * hot-apply/egress-ресинка: эти флаги читает только headless-агентд при старте. Мгновенно.
   * Возвращает нормализованный набор (невалидная autonomy → `null` = confirm).
   */
  setAgentFlags: (flags: AgentFlagsDto): Promise<AgentFlagsDto> =>
    bridge<AgentFlagsDto>('set_agent_flags', { flags }, () => mockSettings.setAgentFlags(flags)),
};

/** Web-агент (W-3): consent-конфиг SearXNG (URL = разрешение на эгресс к нему). Вне Tauri — память. */
export const websearch = {
  getConfig: (): Promise<WebSearchConfig> =>
    bridge<WebSearchConfig>('get_websearch_config', undefined, () =>
      mockSettings.getWebsearchConfig(),
    ),
  setConfig: (config: WebSearchConfig): Promise<WebSearchConfig> =>
    bridge<WebSearchConfig>('set_websearch_config', { config }, () =>
      mockSettings.setWebsearchConfig(config),
    ),
};
