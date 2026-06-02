import i18n from './setup';

/** Число в формате активной (или заданной) локали: 50000 → «50 000» (ru) / «50,000» (en). */
export function formatNumber(value: number, locale: string = i18n.language): string {
  return new Intl.NumberFormat(locale).format(value);
}

const collators = new Map<string, Intl.Collator>();

/** Кэшированный `Intl.Collator` (числовая сортировка, без учёта регистра). */
export function collatorFor(locale: string = i18n.language): Intl.Collator {
  let c = collators.get(locale);
  if (!c) {
    c = new Intl.Collator(locale, { numeric: true, sensitivity: 'base' });
    collators.set(locale, c);
  }
  return c;
}

/** Сортировка узлов дерева: каталоги выше файлов, затем имя через Collator активной локали. */
export function compareEntries(
  a: { isDir: boolean; name: string },
  b: { isDir: boolean; name: string },
): number {
  if (a.isDir !== b.isDir) return a.isDir ? -1 : 1;
  return collatorFor().compare(a.name, b.name);
}
