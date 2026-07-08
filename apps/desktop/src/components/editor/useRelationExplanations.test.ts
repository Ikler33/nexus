import { renderHook, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { tauriApi, type LinkSuggestion } from '../../lib/tauri-api';
import { usePrefsStore } from '../../stores/prefs';
import {
  __resetRelationExplanationsForTest,
  useRelationExplanations,
} from './useRelationExplanations';

const items = (paths: string[]): LinkSuggestion[] =>
  paths.map((p) => ({ path: p, title: null, score: 0.5, reason: `сниппет ${p}` }));

afterEach(() => {
  vi.restoreAllMocks();
  __resetRelationExplanationsForTest();
  usePrefsStore.setState({ aiExplainRelations: true });
});

describe('useRelationExplanations (AIP-10)', () => {
  it('подгружает LLM-объяснения для пар активной заметки', async () => {
    usePrefsStore.setState({ aiExplainRelations: true });
    const spy = vi.spyOn(tauriApi.suggest, 'explainRelation').mockResolvedValue('LLM-причина');
    const { result } = renderHook(() => useRelationExplanations('A.md', items(['X.md', 'Y.md'])));
    await waitFor(() => expect(result.current['X.md']).toBe('LLM-причина'));
    expect(result.current['Y.md']).toBe('LLM-причина');
    expect(spy).toHaveBeenCalledWith('A.md', 'X.md');
    expect(spy).toHaveBeenCalledWith('A.md', 'Y.md');
    expect(spy).toHaveBeenCalledTimes(2); // одна пара = один вызов (дедуп по ключу)
  });

  it('тумблер ВЫКЛ → {} и НЕ шлёт IPC (фолбэк на сниппет)', () => {
    usePrefsStore.setState({ aiExplainRelations: false });
    const spy = vi.spyOn(tauriApi.suggest, 'explainRelation');
    const { result } = renderHook(() => useRelationExplanations('A.md', items(['X.md'])));
    expect(result.current).toEqual({});
    expect(spy).not.toHaveBeenCalled();
  });

  it('пустой ответ (нет модели/ошибка) → ключа нет → карточка покажет сниппет', async () => {
    usePrefsStore.setState({ aiExplainRelations: true });
    vi.spyOn(tauriApi.suggest, 'explainRelation').mockResolvedValue('');
    const { result } = renderHook(() => useRelationExplanations('A.md', items(['X.md'])));
    await new Promise((r) => setTimeout(r, 30));
    expect(result.current['X.md']).toBeUndefined();
  });

  it('ошибка IPC (reject) не валит хук — ключа нет (фолбэк)', async () => {
    usePrefsStore.setState({ aiExplainRelations: true });
    vi.spyOn(tauriApi.suggest, 'explainRelation').mockRejectedValue(new Error('boom'));
    const { result } = renderHook(() => useRelationExplanations('A.md', items(['X.md'])));
    await new Promise((r) => setTimeout(r, 30));
    expect(result.current['X.md']).toBeUndefined();
  });

  it('сам с собой не объясняется (item.path === activePath пропускается)', () => {
    usePrefsStore.setState({ aiExplainRelations: true });
    const spy = vi.spyOn(tauriApi.suggest, 'explainRelation').mockResolvedValue('x');
    renderHook(() => useRelationExplanations('A.md', items(['A.md'])));
    expect(spy).not.toHaveBeenCalled();
  });
});
