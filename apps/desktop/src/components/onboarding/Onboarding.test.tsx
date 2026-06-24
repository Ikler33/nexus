import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { Onboarding } from './Onboarding';
import { tauriApi } from '../../lib/tauri-api';
import * as commandsCore from '../../lib/commands-core';
import { useVaultStore } from '../../stores/vault';
import { useUIStore } from '../../stores/ui';

afterEach(() => {
  vi.restoreAllMocks();
  useVaultStore.setState({ info: null });
  useUIStore.setState({ onboardingActive: false });
});

/** Прокликивает welcome → vault → (открытие vault) → шаг AI. */
async function gotoAiStep() {
  vi.spyOn(commandsCore, 'openVaultFlow').mockImplementation(async () => {
    useVaultStore.setState({ info: { root: '/v', name: 'v' } as never });
  });
  render(<Onboarding />);
  fireEvent.click(screen.getByRole('button', { name: /Начать настройку|Get started|Set up/i }));
  fireEvent.click(screen.getByRole('button', { name: /Открыть папку|Open folder/i }));
  await screen.findByText(/Локальная модель|Local model/i);
}

describe('Onboarding — шаг настройки AI (W-7, ST-A3)', () => {
  it('эндпоинты предзаполнены дефолтами .28, когда конфиг пуст', async () => {
    vi.spyOn(tauriApi.settings, 'getAiConfig').mockResolvedValue({
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
      shellSupported: false,
    });
    await gotoAiStep();
    await waitFor(() =>
      expect(screen.getByDisplayValue('http://192.168.0.28:8080')).toBeInTheDocument(),
    );
    expect(screen.getByDisplayValue('http://192.168.0.28:8083')).toBeInTheDocument();
  });

  it('«Сохранить и проверить» пишет set_ai_config и тестирует связь', async () => {
    vi.spyOn(tauriApi.settings, 'getAiConfig').mockResolvedValue({
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
      shellSupported: false,
    });
    const setCfg = vi
      .spyOn(tauriApi.settings, 'setAiConfig')
      .mockResolvedValue({ chatApplied: true, embeddingChanged: true });
    const probe = vi.spyOn(tauriApi.settings, 'testConnection').mockResolvedValue();
    await gotoAiStep();
    fireEvent.click(await screen.findByRole('button', { name: /Сохранить и проверить|Save & test/i }));
    await waitFor(() =>
      expect(setCfg).toHaveBeenCalledWith(
        { url: 'http://192.168.0.28:8080', model: null },
        { url: 'http://192.168.0.28:8083', model: null },
        null, // fast сохраняется (тут не задан)
      ),
    );
    await waitFor(() => expect(probe).toHaveBeenCalledWith('http://192.168.0.28:8080'));
    // Здоров → пилюля «Готов».
    expect(await screen.findByText(/Готов|Ready/i)).toBeInTheDocument();
    // embeddingChanged=true → подсказка о перезапуске/переиндексации (ревью W-7).
    expect(await screen.findByText(/Перезапустите|Restart the app/i)).toBeInTheDocument();
  });

  // Ревью W-7: сохранение из онбординга НЕ должно стирать существующий ai.fast (data-loss).
  it('существующий ai.fast сохраняется (передаётся 3-м аргументом)', async () => {
    vi.spyOn(tauriApi.settings, 'getAiConfig').mockResolvedValue({
      chat: { url: 'http://h:8080', model: null },
      embedding: null,
      fast: { url: 'http://h:8084', model: 'qwen3-4b' },
      agentAutonomy: null,
      agentActuatorEnabled: false,
      sandboxEnabled: false,
      shellEnable: false,
      webAllowPublicFetch: false,
      skillsLearningEnabled: false,
      agentSkillsDir: null,
      delegationEnabled: false,
      shellSupported: false,
    });
    const setCfg = vi
      .spyOn(tauriApi.settings, 'setAiConfig')
      .mockResolvedValue({ chatApplied: true, embeddingChanged: false });
    vi.spyOn(tauriApi.settings, 'testConnection').mockResolvedValue();
    await gotoAiStep();
    fireEvent.click(await screen.findByRole('button', { name: /Сохранить и проверить|Save & test/i }));
    await waitFor(() =>
      expect(setCfg).toHaveBeenCalledWith(expect.anything(), expect.anything(), {
        url: 'http://h:8084',
        model: 'qwen3-4b',
      }),
    );
  });

  it('существующий конфиг с моделью НЕ затирается на сохранении', async () => {
    vi.spyOn(tauriApi.settings, 'getAiConfig').mockResolvedValue({
      chat: { url: 'http://h:8080', model: 'qwen' },
      embedding: { url: 'http://h:8083', model: 'bge-m3' },
      fast: null,
      agentAutonomy: null,
      agentActuatorEnabled: false,
      sandboxEnabled: false,
      shellEnable: false,
      webAllowPublicFetch: false,
      skillsLearningEnabled: false,
      agentSkillsDir: null,
      delegationEnabled: false,
      shellSupported: false,
    });
    const setCfg = vi
      .spyOn(tauriApi.settings, 'setAiConfig')
      .mockResolvedValue({ chatApplied: true, embeddingChanged: false });
    vi.spyOn(tauriApi.settings, 'testConnection').mockResolvedValue();
    await gotoAiStep();
    expect(await screen.findByDisplayValue('http://h:8080')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: /Сохранить и проверить|Save & test/i }));
    await waitFor(() =>
      expect(setCfg).toHaveBeenCalledWith(
        { url: 'http://h:8080', model: 'qwen' },
        { url: 'http://h:8083', model: 'bge-m3' },
        null,
      ),
    );
  });
});
