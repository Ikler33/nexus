/**
 * Мок политики эгресса (срез 2 net.md) для браузерного dev/vitest: in-memory состояние с той же
 * семантикой, что бэкенд-команды (`get_egress_state`/`set_egress_offline`/`set_egress_feature`) —
 * каждый сеттер возвращает свежий снимок.
 */
import type { EgressFeatureId, EgressState } from '../tauri-api';

let state: EgressState = { offline: false, chat: true, embed: true, probe: true };

export async function getState(): Promise<EgressState> {
  return { ...state };
}

export async function setOffline(offline: boolean): Promise<EgressState> {
  state = { ...state, offline };
  return { ...state };
}

export async function setFeature(feature: EgressFeatureId, enabled: boolean): Promise<EgressState> {
  state = { ...state, [feature]: enabled };
  return { ...state };
}
