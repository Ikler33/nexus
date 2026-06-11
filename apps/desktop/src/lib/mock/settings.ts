import type { AiConfigDto, AiEndpoint, SetAiResult } from '../tauri-api';

/**
 * Мок раздела настроек «AI / Модели» для браузерного превью / vitest (вне Tauri).
 * Хранит конфиг в памяти; реальная логика (запись `.nexus/local.json` + hot-apply chat +
 * проверка связи) — в Rust `commands/settings.rs`. Здесь — happy-path для UI/тестов.
 */

let config: AiConfigDto = { chat: null, embedding: null, fast: null };

export async function getAiConfig(): Promise<AiConfigDto> {
  return { chat: config.chat, embedding: config.embedding, fast: config.fast };
}

export async function setAiConfig(
  chat: AiEndpoint | null,
  embedding: AiEndpoint | null,
  fast: AiEndpoint | null = null,
): Promise<SetAiResult> {
  const embeddingChanged = JSON.stringify(config.embedding) !== JSON.stringify(embedding);
  config = { chat, embedding, fast };
  return { chatApplied: true, embeddingChanged };
}

export async function testConnection(url: string): Promise<void> {
  if (!/^https?:\/\/.+/.test(url.trim())) throw new Error('некорректный URL (ожидается http(s)://…)');
  // В превью любой синтаксически верный URL считаем достижимым.
}
