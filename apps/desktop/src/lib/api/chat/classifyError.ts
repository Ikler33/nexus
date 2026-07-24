/**
 * U5 first-run honesty: map backend/pre-stream failures to typed chat banners.
 * Backend `chat_rag` returns Err (no stream event) when `ai.chat` is missing —
 * FE must not leave a forever-thinking bubble or a raw unhelpful string only.
 */

import type { EgressDeniedKind } from './types';

/** Russian backend string from commands/chat.rs + possible Tauri wrappers. */
const AI_MISSING_RE =
  /chat-провайдер\s+не\s+сконфигурирован|chat\s*provider\s+not\s+configur|ai\.chat|\.nexus\/local\.json\s*→\s*ai\.chat/i;

export function classifyChatInvokeError(err: unknown): {
  message: string;
  deniedKind?: EgressDeniedKind;
} {
  const message = err instanceof Error ? err.message : String(err);
  if (AI_MISSING_RE.test(message)) {
    return { message, deniedKind: 'aiMissing' };
  }
  return { message };
}

export function isAiMissingMessage(message: string): boolean {
  return AI_MISSING_RE.test(message);
}
