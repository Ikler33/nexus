import { create } from 'zustand';

import { tauriApi } from '../lib/tauri-api';

/**
 * Тогглы фоновых ИИ-фич Home, гейтируемых владельцем (real-test 2026-06-18): «Инсайты» (открытые
 * вопросы + дрейф контекста + stale-radar) и «Поиск противоречий». Оба дефолт **OFF** (opt-in) — на
 * reference/MOC-vault'ах дают пусто/нишево, не гоняем фон по умолчанию.
 *
 * Источник истины — БД vault (persisted `insights.enabled` / `contradictions.enabled`), как у
 * `episodic.enabled`. Поэтому НЕ держим в localStorage (иначе дефолт-OFF на новой машине разошёлся бы с
 * включённым в БД → privacy-десинк, урок EP-3): грузим состояние от бэка при каждом открытии vault.
 */
interface AiFeaturesState {
  insights: boolean;
  contradictions: boolean;
  /** Подтянуть оба persisted-флага от бэка (на открытии vault). Best-effort: ошибка → оставляем как есть. */
  sync: () => Promise<void>;
  /** Переключить «Инсайты» (ВКЛ enqueue'ит kick-генерацию доступных виджетов на бэке). */
  setInsights: (on: boolean) => Promise<void>;
  /** Переключить «Поиск противоречий» (ВКЛ enqueue'ит kick-джобу на бэке). */
  setContradictions: (on: boolean) => Promise<void>;
}

let syncSeq = 0;

export const useAiFeaturesStore = create<AiFeaturesState>((set) => ({
  insights: false,
  contradictions: false,
  async sync() {
    // Монотонный токен против гонок при быстрой смене vault (как episode/home-стор). При ошибке/смене
    // vault сбрасываем к дефолту OFF, а НЕ наследуем значения прошлого vault (privacy-класс EP-3:
    // лучше ложно-OFF, чем показать ON от чужого vault). Применяем только САМЫЙ свежий ответ.
    const seq = ++syncSeq;
    try {
      const [insights, contradictions] = await Promise.all([
        tauriApi.home.insightsGetEnabled(),
        tauriApi.contradictions.getEnabled(),
      ]);
      if (seq === syncSeq) set({ insights, contradictions });
    } catch {
      if (seq === syncSeq) set({ insights: false, contradictions: false });
    }
  },
  async setInsights(on) {
    await tauriApi.home.insightsSetEnabled(on);
    set({ insights: on });
  },
  async setContradictions(on) {
    await tauriApi.contradictions.setEnabled(on);
    set({ contradictions: on });
  },
}));
