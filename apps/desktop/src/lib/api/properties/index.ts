import * as mockProps from '../../mock/properties';
import { bridge } from '../bridge';
import type { NoteProperty, PropertyType } from './types';

/**
 * Properties-домен (F-2d): реестр типов свойств (PROP-2, Obsidian Properties) — тип глобален по имени,
 * иначе эвристика; свойства заметки для Properties-панели (PROP-3). Все вызовы — через `bridge`
 * (Tauri ↔ мок `lib/mock/properties`); потребители ходят сюда по-прежнему через `tauriApi.properties`
 * (barrel-реэкспорт в `lib/tauri-api.ts`).
 */
export const properties = {
  /** Весь реестр явных типов (имя → тип). */
  types: (): Promise<Record<string, PropertyType>> =>
    bridge<Record<string, PropertyType>>('get_property_types', undefined, () => mockProps.types()),
  /** Задать явный тип свойства (глобально по имени). */
  setType: (key: string, type: PropertyType): Promise<void> =>
    bridge<void>('set_property_type', { key, ty: type }, () => mockProps.setType(key, type)),
  /** Свойства заметки с разрешённым типом (для Properties-панели PROP-3). */
  forNote: (path: string): Promise<NoteProperty[]> =>
    bridge<NoteProperty[]>('get_note_properties', { path }, () => mockProps.forNote()),
};
