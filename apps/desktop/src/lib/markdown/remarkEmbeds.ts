import type { Paragraph, Root } from 'mdast';
import { visit } from 'unist-util-visit';

import { EMBED_PARAGRAPH_RE, isImageTarget, parseEmbedTarget } from './embed';

/** Минимальная форма VFile (без прямой зависимости от пакета `vfile`): нужен только исходный текст. */
type SourceFile = { toString(): string };

/** Потолок числа блок-вставок на ОДНУ заметку (ревью транклюзии, нит fan-out): защита от патологического
 *  сгенерированного списка `![[…]]` × тысячи. 50 заведомо выше реального MOC/индекса (10–20); сверх —
 *  падают в прежнее `!`+вики-ссылку. Рекурсию ограничивает отдельный `MAX_EMBED_DEPTH`. */
const MAX_EMBEDS_PER_NOTE = 50;

/**
 * remark-плагин транклюзии: абзац, который ЦЕЛИКОМ (после trim) равен `![[ … ]]`, заменяется на
 * кастомный узел `nexus-embed` (через `data.hName`/`hProperties` → элемент в hast), который
 * `MarkdownPreview` рендерит компонентом `NoteEmbed` (рекурсивная вставка заметки).
 *
 * Почему по offset'ам исходника, а не по тексту узлов: `![[note]]` в mdast может разложиться нетривиально
 * (вложенные `linkReference`-узлы от `[ … ]`), поэтому берём ТОЧНЫЙ срез исходника по `node.position`
 * (react-markdown кладёт исходник в `VFile`) и матчим регэкспом — без зависимости от токенизации.
 *
 * Охват слайса: блок-вставка (абзац = один `![[…]]`). Инлайн-вставка в середине текста и несколько
 * `![[…]]` в одном абзаце (через мягкие переводы строк) НЕ матчатся — падают в прежнее поведение
 * (`!` + `[[wikilink]]` из remarkNexus), без регрессии. Картинки `![[pic.png]]` тоже пропускаются.
 */
export function remarkEmbeds() {
  return (tree: Root, file: SourceFile): void => {
    const src = String(file);
    let emitted = 0;
    visit(tree, 'paragraph', (node: Paragraph, index, parent) => {
      if (emitted >= MAX_EMBEDS_PER_NOTE) return; // потолок fan-out на заметку — остальные как `!`+вики
      if (index == null || !parent || !node.position) return;
      const start = node.position.start.offset;
      const end = node.position.end.offset;
      if (start == null || end == null) return;
      const raw = src.slice(start, end).trim();
      const m = EMBED_PARAGRAPH_RE.exec(raw);
      if (!m) return;
      const { note, anchor } = parseEmbedTarget(m[1]);
      if (note.length === 0 || isImageTarget(note)) return; // пусто (`![[#H]]`) / картинка — не наш случай
      // Кастомный узел: пустые children + hName → mdast-util-to-hast создаёт <nexus-embed>, который
      // react-markdown рендерит через components['nexus-embed']. Полезную нагрузку (target/anchor)
      // читаем в компоненте из `node.properties` (hProperties копируются туда дословно).
      parent.children[index] = {
        type: 'paragraph', // тип игнорируется — рендер идёт по data.hName; держим валидный mdast-узел
        children: [],
        data: {
          hName: 'nexus-embed',
          hProperties: { target: note, anchor: anchor ?? '' },
        },
      } as Paragraph;
      emitted += 1;
    });
  };
}
