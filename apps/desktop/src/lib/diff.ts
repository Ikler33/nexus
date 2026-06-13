// Построчный diff (SAFE-6: история версий / сравнение буфера с диском). Чистая функция без
// зависимостей — LCS по строкам. Для огромных файлов есть грубый фолбэк (полная замена).

export type DiffLine = { type: 'same' | 'add' | 'del'; text: string };

/** Порог n×m, выше которого LCS-таблица слишком тяжёлая → грубый фолбэк (всё удалено + всё добавлено). */
const LCS_CELL_LIMIT = 4_000_000;

/**
 * Построчный diff через LCS. `a` — опорная сторона (старая версия), `b` — текущая (буфер).
 * `del` = строка есть в `a`, но не в `b`; `add` = есть в `b`, но не в `a`; `same` = общая.
 */
export function lineDiff(a: string, b: string): DiffLine[] {
  const A = a.split('\n');
  const B = b.split('\n');
  const n = A.length;
  const m = B.length;

  // Защита от тяжёлой DP-таблицы на гигантских файлах.
  if (n * m > LCS_CELL_LIMIT) {
    return [
      ...A.map((text): DiffLine => ({ type: 'del', text })),
      ...B.map((text): DiffLine => ({ type: 'add', text })),
    ];
  }

  // dp[i][j] — длина LCS суффиксов A[i..] и B[j..].
  const dp: number[][] = Array.from({ length: n + 1 }, () => new Array<number>(m + 1).fill(0));
  for (let i = n - 1; i >= 0; i--) {
    for (let j = m - 1; j >= 0; j--) {
      dp[i][j] =
        A[i] === B[j] ? dp[i + 1][j + 1] + 1 : Math.max(dp[i + 1][j], dp[i][j + 1]);
    }
  }

  const out: DiffLine[] = [];
  let i = 0;
  let j = 0;
  while (i < n && j < m) {
    if (A[i] === B[j]) {
      out.push({ type: 'same', text: A[i] });
      i++;
      j++;
    } else if (dp[i + 1][j] >= dp[i][j + 1]) {
      out.push({ type: 'del', text: A[i] });
      i++;
    } else {
      out.push({ type: 'add', text: B[j] });
      j++;
    }
  }
  while (i < n) out.push({ type: 'del', text: A[i++] });
  while (j < m) out.push({ type: 'add', text: B[j++] });
  return out;
}

/** Сводка изменений: сколько строк добавлено/удалено. */
export function diffStat(diff: DiffLine[]): { added: number; removed: number } {
  let added = 0;
  let removed = 0;
  for (const d of diff) {
    if (d.type === 'add') added++;
    else if (d.type === 'del') removed++;
  }
  return { added, removed };
}
