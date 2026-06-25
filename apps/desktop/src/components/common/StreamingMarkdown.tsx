import { useEffect, useRef, useState } from 'react';

import { closeOpenFences } from '../../lib/markdown/closeOpenFences';
import { Markdown } from './Markdown';

/**
 * W-34: live-рендер markdown по ходу стрима. Троттл ре-парса (~90мс) + толерантность к недописанному
 * синтаксису (closeOpenFences). Mermaid ОТКЛЮЧЁН (`mermaid={false}`) — частичная диаграмма мигала бы;
 * финальный рендер (`Markdown` с дефолтным `mermaid`) отрисует её целиком. При завершении стрима
 * ChatView/AgentView переключаются на полный `Markdown` — слегка устаревший последний стрим-кадр
 * заменяется финалом, троттл лишь коалесцирует частые токены.
 */
export function StreamingMarkdown({ text, className }: { text: string; className?: string }) {
  const [shown, setShown] = useState(text);
  const latest = useRef(text);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  useEffect(() => {
    latest.current = text;
    if (timer.current != null) return; // уже запланировано — коалесцируем
    timer.current = setTimeout(() => {
      timer.current = null;
      setShown(latest.current);
    }, 90);
  }, [text]);
  useEffect(
    () => () => {
      if (timer.current != null) clearTimeout(timer.current);
    },
    [],
  );
  return <Markdown content={closeOpenFences(shown)} mermaid={false} className={className} />;
}
