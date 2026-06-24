import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { tauriApi } from '../../lib/tauri-api';
import { useVaultStore } from '../../stores/vault';
import { SelfCheck } from './SelfCheck';

afterEach(() => {
  vi.restoreAllMocks();
  useVaultStore.setState({ info: null });
});

const ENDPOINT = (url: string, model: string | null = null) => ({ url, model });

describe('SelfCheck — dev self-check (W-21)', () => {
  it('все эндпоинты достижимы → ✓, показывает URL/модель', async () => {
    vi.spyOn(tauriApi.settings, 'getAiConfig').mockResolvedValue({
      chat: ENDPOINT('http://192.168.0.28:8080', 'qwen'),
      embedding: ENDPOINT('http://192.168.0.28:8083', 'bge-m3'),
      fast: ENDPOINT('http://192.168.0.28:8084', null),
      agentAutonomy: null,
      agentActuatorEnabled: false,
      sandboxEnabled: false,
      shellEnable: false,
      webAllowPublicFetch: false,
      skillsLearningEnabled: false,
      agentSkillsDir: null,
      delegationEnabled: false,
      researchEnabled: false,
      shellSupported: false,
    });
    const probe = vi.spyOn(tauriApi.settings, 'testConnection').mockResolvedValue();
    render(<SelfCheck />);
    await waitFor(() => expect(screen.getAllByText('✓').length).toBe(3));
    expect(probe).toHaveBeenCalledWith('http://192.168.0.28:8080');
    expect(screen.getByText(/192\.168\.0\.28:8080 · qwen/)).toBeInTheDocument();
  });

  it('недостижимый эндпоинт (дрейф .31) → ✗ + текст ошибки', async () => {
    vi.spyOn(tauriApi.settings, 'getAiConfig').mockResolvedValue({
      chat: ENDPOINT('http://192.168.0.31:8080', 'gemma'),
      embedding: ENDPOINT('http://192.168.0.28:8083', 'bge-m3'),
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
      shellSupported: false,
    });
    vi.spyOn(tauriApi.settings, 'testConnection').mockImplementation(async (url: string) => {
      if (url.includes('0.31')) throw new Error('нет связи с моделью');
      return undefined;
    });
    render(<SelfCheck />);
    await waitFor(() => expect(screen.getByText('✗')).toBeInTheDocument());
    expect(screen.getByText(/нет связи с моделью/)).toBeInTheDocument();
    // fast не задан → нейтральный «—», не ошибка.
    expect(screen.getByText('—')).toBeInTheDocument();
  });

  it('кнопка «Скрыть» убирает карточку', async () => {
    vi.spyOn(tauriApi.settings, 'getAiConfig').mockResolvedValue({
      chat: ENDPOINT('http://192.168.0.28:8080'),
      embedding: ENDPOINT('http://192.168.0.28:8083'),
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
      shellSupported: false,
    });
    vi.spyOn(tauriApi.settings, 'testConnection').mockResolvedValue();
    render(<SelfCheck />);
    const dismiss = await screen.findByRole('button', { name: /Скрыть|Dismiss/ });
    fireEvent.click(dismiss);
    await waitFor(() => expect(screen.queryByText(/Самопроверка|Self-check/)).toBeNull());
  });

  it('смена vault → перепроверяет заново (дрейф в новом vault не остаётся скрытым)', async () => {
    const getCfg = vi.spyOn(tauriApi.settings, 'getAiConfig').mockResolvedValue({
      chat: ENDPOINT('http://192.168.0.28:8080'),
      embedding: ENDPOINT('http://192.168.0.28:8083'),
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
      shellSupported: false,
    });
    vi.spyOn(tauriApi.settings, 'testConnection').mockResolvedValue();
    render(<SelfCheck />);
    await waitFor(() => expect(getCfg).toHaveBeenCalledTimes(1));
    act(() => {
      useVaultStore.setState({ info: { root: '/other/vault' } as never });
    });
    await waitFor(() => expect(getCfg).toHaveBeenCalledTimes(2));
  });

  it('vault не открыт (getAiConfig падает) → карточка не рисуется', async () => {
    vi.spyOn(tauriApi.settings, 'getAiConfig').mockRejectedValue(new Error('no vault'));
    render(<SelfCheck />);
    await waitFor(() => expect(tauriApi.settings.getAiConfig).toHaveBeenCalled());
    expect(screen.queryByText(/Самопроверка|Self-check/)).toBeNull();
  });
});
