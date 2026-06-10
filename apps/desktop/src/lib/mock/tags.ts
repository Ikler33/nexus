/** Мок тегов vault (DP-2) для браузерного dev/vitest — панель «Теги» сайдбара. */
import type { TagCount } from '../tauri-api';

export async function listTags(): Promise<TagCount[]> {
  return [
    { name: 'ai', count: 12 },
    { name: 'project', count: 8 },
    { name: 'rag', count: 5 },
    { name: 'planning', count: 3 },
    { name: 'идеи', count: 2 },
  ];
}
