import { describe, expect, it } from 'vitest';
import i18n, { detectLocale } from './setup';
import { compareEntries, formatNumber } from './format';
import en from './en.json';
import ru from './ru.json';

type Json = { [k: string]: string | Json };

/** Базовые ключи без плюрал-суффиксов (для сравнения наборов ru/en). */
function baseKeys(obj: Json, prefix = ''): Set<string> {
  const out = new Set<string>();
  for (const [k, v] of Object.entries(obj)) {
    const key = prefix ? `${prefix}.${k}` : k;
    if (v && typeof v === 'object') {
      for (const bk of baseKeys(v, key)) out.add(bk);
    } else {
      out.add(key.replace(/_(one|few|many|other|two)$/, ''));
    }
  }
  return out;
}

describe('i18n (Ф0-10)', () => {
  it('AC-I18N-1: ru и en имеют одинаковый набор ключей', () => {
    expect([...baseKeys(ru as Json)].sort()).toEqual([...baseKeys(en as Json)].sort());
  });

  it('AC-I18N-2: русские плюралы one/few/many', async () => {
    await i18n.changeLanguage('ru');
    expect(i18n.t('backlinks.count', { count: 1 })).toBe('1 беклинк');
    expect(i18n.t('backlinks.count', { count: 2 })).toBe('2 беклинка');
    expect(i18n.t('backlinks.count', { count: 5 })).toBe('5 беклинков');
  });

  it('AC-I18N-3: числа форматируются через Intl под локаль', () => {
    expect(formatNumber(50000, 'en')).toBe('50,000');
    expect(formatNumber(50000, 'ru')).not.toBe('50000'); // ru группирует разряды
  });

  it('AC-I18N-4: сортировка через Intl.Collator (каталоги выше, кириллица)', () => {
    const items = [
      { isDir: false, name: 'яблоко.md' },
      { isDir: true, name: 'Заметки' },
      { isDir: false, name: 'Ананас.md' },
      { isDir: true, name: 'Архив' },
    ];
    const sorted = [...items].sort(compareEntries).map((i) => i.name);
    expect(sorted.slice(0, 2)).toEqual(['Архив', 'Заметки']); // каталоги первыми, по алфавиту
    expect(sorted.slice(2)).toEqual(['Ананас.md', 'яблоко.md']); // файлы: А < я
  });

  it('AC-I18N-5: детекция локали и смена языка', async () => {
    expect(['ru', 'en']).toContain(detectLocale());
    await i18n.changeLanguage('en');
    expect(i18n.t('app.openVault')).toBe('Open vault…');
    await i18n.changeLanguage('ru');
    expect(i18n.t('app.openVault')).toBe('Открыть vault…');
  });
});
