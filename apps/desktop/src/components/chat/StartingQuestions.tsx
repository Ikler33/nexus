import { useEffect, useState } from 'react';
import { tauriApi } from '../../lib/tauri-api';
import { getCachedQuestions, setCachedQuestions } from './startingQuestionsCache';
import styles from './ChatPanel.module.css';

/**
 * AIP-SQ: подсказки-пилюли в ПУСТОМ чате. Если открыта заметка (`center`) — спрашиваем у бэка
 * контекстные вопросы по ней (LLM `chat_util`, best-effort); пока их нет / пусто / нет заметки —
 * показываем статические `staticPills` (грациозная деградация, пустого блока не бывает). Клик → onAsk.
 * Кэш — session-level по пути заметки (см. `startingQuestionsCache`): повтор той же заметки не зовёт LLM.
 */

export function StartingQuestions({
  center,
  staticPills,
  onAsk,
}: {
  center: string | null;
  staticPills: string[];
  onAsk: (q: string) => void;
}) {
  const [dynamic, setDynamic] = useState<string[]>(() =>
    center ? (getCachedQuestions(center) ?? []) : [],
  );

  useEffect(() => {
    // Нет активной заметки → только статика, без LLM-вызова (экономим бюджет).
    if (!center) {
      setDynamic([]);
      return;
    }
    const cached = getCachedQuestions(center);
    if (cached) {
      setDynamic(cached);
      return;
    }
    // Гасим вопросы прошлой заметки СРАЗУ — иначе при смене center A→B (B ещё не в кэше) они мелькнут
    // под заголовком B до резолва (компонент не перемонтируется). Зеркало ChatView suggest-среза (AIP-11).
    setDynamic([]);
    let alive = true;
    tauriApi.suggest
      .startingQuestions(center)
      .then((qs) => {
        setCachedQuestions(center, qs); // кэшируем даже пустой ответ — не дёргать LLM снова за сессию
        if (alive) setDynamic(qs);
      })
      .catch(() => {
        if (alive) setDynamic([]);
      });
    return () => {
      alive = false; // смена файла до резолва → игнорируем ответ по неактуальному center
    };
  }, [center]);

  // Динамические вопросы заменяют статику, как только пришли непустыми; иначе — статические подсказки.
  const items = dynamic.length > 0 ? dynamic : staticPills;

  return (
    <div className={styles.suggestPills}>
      {items.map((p) => (
        <button key={p} type="button" className={styles.suggestPill} onClick={() => onAsk(p)}>
          {p}
        </button>
      ))}
    </div>
  );
}
