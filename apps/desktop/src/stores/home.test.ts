import { afterEach, describe, expect, it, vi } from 'vitest';

import { useHomeStore } from './home';
import { useToastStore } from './toast';
import { tauriApi } from '../lib/tauri-api';

afterEach(() => {
  vi.restoreAllMocks();
  useToastStore.setState({ toasts: [] });
});

describe('home store: syncGenerating (AIP-5 — честный «генерирю…»)', () => {
  it('активная home_widget:/stale_radar-джоба (running/готовая) → generating[ключ]=true', async () => {
    vi.spyOn(tauriApi.scheduler, 'activeJobs').mockResolvedValue([
      { id: 1, kind: 'home_widget:open_questions', state: 'running', runAt: 0, attempts: 1 },
      { id: 2, kind: 'home_widget:context_drift', state: 'pending', runAt: 0, attempts: 0 },
      { id: 3, kind: 'stale_radar', state: 'running', runAt: 0, attempts: 0 },
      { id: 4, kind: 'newsfeed', state: 'pending', runAt: 0, attempts: 0 },
    ]);
    useHomeStore.setState({ generating: {} });
    await useHomeStore.getState().syncGenerating();
    const g = useHomeStore.getState().generating;
    expect(g.open_questions).toBe(true);
    expect(g.context_drift).toBe(true);
    expect(g.stale_radar).toBe(true); // AIP-хвост — отдельный kind, тот же индикатор
    expect(g.newsfeed).toBeUndefined(); // не home_widget:/stale_radar — не трогаем
  });

  // Adversarial-ревью: future-pending recurring-джоба (переарм после прогона) НЕ должна считаться
  // «генерируется» — иначе спиннер залипал бы после снятия (класс #63 ready-vs-future).
  it('future-pending recurring-джоба → НЕ ставит флаг', async () => {
    const future = Math.floor(Date.now() / 1000) + 24 * 3600;
    vi.spyOn(tauriApi.scheduler, 'activeJobs').mockResolvedValue([
      { id: 1, kind: 'home_widget:open_questions', state: 'pending', runAt: future, attempts: 0 },
      { id: 2, kind: 'stale_radar', state: 'pending', runAt: future, attempts: 0 },
    ]);
    useHomeStore.setState({ generating: {} });
    await useHomeStore.getState().syncGenerating();
    const g = useHomeStore.getState().generating;
    expect(g.open_questions).toBeUndefined();
    expect(g.stale_radar).toBeUndefined();
  });

  it('только ДОБАВЛЯЕТ флаги (снятие — по widget-updated), не сбрасывает чужие', async () => {
    vi.spyOn(tauriApi.scheduler, 'activeJobs').mockResolvedValue([]);
    useHomeStore.setState({ generating: { context_drift: true } });
    await useHomeStore.getState().syncGenerating();
    expect(useHomeStore.getState().generating.context_drift).toBe(true); // не снят, хотя джоб нет
  });

  it('ошибка activeJobs (нет планировщика) → no-op без краша', async () => {
    vi.spyOn(tauriApi.scheduler, 'activeJobs').mockRejectedValue(new Error('no scheduler'));
    useHomeStore.setState({ generating: {} });
    await expect(useHomeStore.getState().syncGenerating()).resolves.toBeUndefined();
    expect(useHomeStore.getState().generating).toEqual({});
  });
});

// P0-4: одно поле `error` раньше писалось И фатальным load(), И пер-виджетным reload/refresh →
// провал «Обновить» одного виджета вешал глобальный баннер поверх рабочего дашборда. Теперь:
// фатальный load() → баннер; пер-виджетный провал → тост (НЕ глобальный error); успех сбрасывает error.
describe('home store: расщепление error фатальный/пер-виджетный (P0-4)', () => {
  it('фатальный load-fail → глобальный error (баннер)', async () => {
    vi.spyOn(tauriApi.home, 'data').mockRejectedValue(new Error('бэкенд лёг'));
    useHomeStore.setState({ error: null });
    await useHomeStore.getState().load();
    expect(useHomeStore.getState().error).toContain('бэкенд лёг');
    expect(useHomeStore.getState().loading).toBe(false);
  });

  it('пер-виджетный reload-fail → НЕ глобальный error, а тост', async () => {
    vi.spyOn(tauriApi.home, 'openQuestions').mockRejectedValue(new Error('виджет упал'));
    useHomeStore.setState({ error: null, generating: { open_questions: true } });
    await useHomeStore.getState().reloadWidget('open_questions');
    // Глобальный баннер НЕ взведён (рабочий дашборд не перекрыт ошибкой одного виджета).
    expect(useHomeStore.getState().error).toBeNull();
    // Спиннер «генерирую…» снят (не залипает, обещая результат).
    expect(useHomeStore.getState().generating.open_questions).toBe(false);
    // Ошибка ВИДИМА локально — тост.
    const toasts = useToastStore.getState().toasts;
    expect(toasts).toHaveLength(1);
    expect(toasts[0].kind).toBe('error');
  });

  it('пер-виджетный refresh-fail → НЕ глобальный error, а тост', async () => {
    vi.spyOn(tauriApi.home, 'refresh').mockRejectedValue(new Error('refresh упал'));
    useHomeStore.setState({ error: null, generating: {} });
    await useHomeStore.getState().refreshWidget('context_drift');
    expect(useHomeStore.getState().error).toBeNull();
    expect(useHomeStore.getState().generating.context_drift).toBe(false);
    expect(useToastStore.getState().toasts).toHaveLength(1);
    expect(useToastStore.getState().toasts[0].kind).toBe('error');
  });

  it('успешный reloadWidget сбрасывает прежний фатальный error', async () => {
    vi.spyOn(tauriApi.home, 'openQuestions').mockResolvedValue([]);
    useHomeStore.setState({ error: 'старый фатальный баннер', generating: {} });
    await useHomeStore.getState().reloadWidget('open_questions');
    expect(useHomeStore.getState().error).toBeNull(); // баннер снят — дашборд снова в порядке
  });
});
