import { describe, expect, it } from 'vitest';

import { classifyChatInvokeError } from './classifyError';

describe('classifyChatInvokeError (U5)', () => {
  it('maps Russian backend missing-provider string → aiMissing', () => {
    const r = classifyChatInvokeError(
      'chat-провайдер не сконфигурирован (.nexus/local.json → ai.chat)',
    );
    expect(r.deniedKind).toBe('aiMissing');
    expect(r.message).toMatch(/chat-провайдер/);
  });

  it('maps Error wrapper the same way', () => {
    const r = classifyChatInvokeError(
      new Error('chat-провайдер не сконфигурирован (.nexus/local.json → ai.chat)'),
    );
    expect(r.deniedKind).toBe('aiMissing');
  });

  it('leaves network errors without deniedKind', () => {
    const r = classifyChatInvokeError('connection refused');
    expect(r.deniedKind).toBeUndefined();
    expect(r.message).toBe('connection refused');
  });
});
