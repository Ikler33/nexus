/** Unix-секунды → относительное время в локали UI («3 ч назад», DP-15: общий хелпер
 * для Home и doc-meta редактора). Старше месяца — короткая календарная дата. */
export function relTime(ts: number, locale: string): string {
  // Знаковая разница: >0 — прошлое, <0 — будущее. Раньше Math.max(0,…) обнулял будущее → ts из будущего
  // показывался «1 мин назад» (находка аудита). rtf-аргумент: прошлое отрицательный, будущее положительный.
  const diff = Math.floor(Date.now() / 1000) - ts;
  const abs = Math.abs(diff);
  const sign = diff >= 0 ? -1 : 1;
  const rtf = new Intl.RelativeTimeFormat(locale, { numeric: 'auto', style: 'short' });
  if (abs < 90) return rtf.format(sign, 'minute');
  if (abs < 3600) return rtf.format(sign * Math.floor(abs / 60), 'minute');
  if (abs < 86_400) return rtf.format(sign * Math.floor(abs / 3600), 'hour');
  if (abs < 30 * 86_400) return rtf.format(sign * Math.floor(abs / 86_400), 'day');
  return new Date(ts * 1000).toLocaleDateString(locale, { day: 'numeric', month: 'short' });
}
