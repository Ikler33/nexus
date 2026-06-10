/** Unix-секунды → относительное время в локали UI («3 ч назад», DP-15: общий хелпер
 * для Home и doc-meta редактора). Старше месяца — короткая календарная дата. */
export function relTime(ts: number, locale: string): string {
  const diff = Math.max(0, Math.floor(Date.now() / 1000) - ts);
  const rtf = new Intl.RelativeTimeFormat(locale, { numeric: 'auto', style: 'short' });
  if (diff < 90) return rtf.format(-1, 'minute');
  if (diff < 3600) return rtf.format(-Math.floor(diff / 60), 'minute');
  if (diff < 86_400) return rtf.format(-Math.floor(diff / 3600), 'hour');
  if (diff < 30 * 86_400) return rtf.format(-Math.floor(diff / 86_400), 'day');
  return new Date(ts * 1000).toLocaleDateString(locale, { day: 'numeric', month: 'short' });
}
