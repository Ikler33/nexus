// logic.jsx — backlinks, markdown rendering, mock AI answers/suggestions.
(function () {
  const NOTES = window.NEXUS_NOTES, TITLE2ID = window.NEXUS_TITLE2ID;
  const WIKILINK = /\[\[([^\]]+)\]\]/g;

  // ----- backlinks: notes that [[link]] to this note's title -----
  function computeBacklinks(noteId) {
    const target = NOTES[noteId]; if (!target) return [];
    const t = target.title.toLowerCase();
    const out = [];
    Object.entries(NOTES).forEach(([id, n]) => {
      if (id === noteId) return;
      const lines = n.body.split("\n");
      lines.forEach((line) => {
        let m; WIKILINK.lastIndex = 0;
        while ((m = WIKILINK.exec(line))) {
          if (m[1].toLowerCase() === t) {
            out.push({ id, title: n.title, context: line.replace(/^#+\s*/, "").trim() });
            break;
          }
        }
      });
    });
    return out;
  }
  window.computeBacklinks = computeBacklinks;

  // ----- inline render: wikilinks, #tags, `code`, **bold** -----
  function renderInline(text, key, { onLink, onTag }) {
    const nodes = []; let i = 0, last = 0;
    // combined tokenizer
    const re = /(\[\[[^\]]+\]\])|(`[^`]+`)|(\*\*[^*]+\*\*)|(#[\wа-яё-]+)/giu;
    let m;
    while ((m = re.exec(text))) {
      if (m.index > last) nodes.push(text.slice(last, m.index));
      const tok = m[0];
      if (tok.startsWith("[[")) {
        const name = tok.slice(2, -2);
        const id = TITLE2ID[name.toLowerCase()];
        nodes.push(React.createElement("span", {
          key: key + "-l" + i, className: "wikilink" + (id ? "" : " wikilink-broken"),
          onClick: (e) => { e.stopPropagation(); if (id && onLink) onLink(id); },
          role: "link", tabIndex: 0,
          onKeyDown: (e) => { if (e.key === "Enter" && id && onLink) onLink(id); },
        }, name));
      } else if (tok.startsWith("`")) {
        nodes.push(React.createElement("code", { key: key + "-c" + i, className: "md-code" }, tok.slice(1, -1)));
      } else if (tok.startsWith("**")) {
        nodes.push(React.createElement("strong", { key: key + "-b" + i }, tok.slice(2, -2)));
      } else if (tok.startsWith("#")) {
        nodes.push(React.createElement("span", {
          key: key + "-t" + i, className: "md-tag",
          onClick: (e) => { e.stopPropagation(); if (onTag) onTag(tok.slice(1)); },
          role: "button", tabIndex: 0,
        }, tok));
      }
      last = re.lastIndex; i++;
    }
    if (last < text.length) nodes.push(text.slice(last));
    return nodes;
  }

  // ----- block render -----
  function renderMarkdown(body, handlers) {
    const lines = body.split("\n");
    const out = []; let li = [];
    const flush = () => {
      if (li.length) { out.push(React.createElement("ul", { key: "ul" + out.length, className: "md-ul" }, li)); li = []; }
    };
    lines.forEach((raw, idx) => {
      const line = raw;
      if (/^#\s/.test(line)) { flush(); out.push(React.createElement("h1", { key: idx, className: "md-h1" }, renderInline(line.slice(2), "h" + idx, handlers))); }
      else if (/^##\s/.test(line)) { flush(); out.push(React.createElement("h2", { key: idx, className: "md-h2" }, renderInline(line.slice(3), "h" + idx, handlers))); }
      else if (/^###\s/.test(line)) { flush(); out.push(React.createElement("h3", { key: idx, className: "md-h3" }, renderInline(line.slice(4), "h" + idx, handlers))); }
      else if (/^>\s/.test(line)) { flush(); out.push(React.createElement("blockquote", { key: idx, className: "md-quote" }, renderInline(line.slice(2), "q" + idx, handlers))); }
      else if (/^(\s*)[-*]\s/.test(line)) { li.push(React.createElement("li", { key: idx, className: "md-li" }, renderInline(line.replace(/^\s*[-*]\s/, ""), "li" + idx, handlers))); }
      else if (/^(\s*)\d+\.\s/.test(line)) { li.push(React.createElement("li", { key: idx, className: "md-li md-li-num", "data-num": line.match(/(\d+)\./)[1] }, renderInline(line.replace(/^\s*\d+\.\s/, ""), "li" + idx, handlers))); }
      else if (line.trim() === "") { flush(); out.push(React.createElement("div", { key: idx, className: "md-gap" })); }
      else { flush(); out.push(React.createElement("p", { key: idx, className: "md-p" }, renderInline(line, "p" + idx, handlers))); }
    });
    flush();
    return out;
  }
  window.renderMarkdown = renderMarkdown;
  window.renderInline = renderInline;

  // ----- mock AI answer keyed loosely by query -----
  function mockAnswer(query, lang) {
    const q = (query || "").toLowerCase();
    let key = "default";
    if (/rag|retriev|поиск|контекст/.test(q)) key = "rag";
    else if (/embed|вектор|косин/.test(q)) key = "embed";
    else if (/local|local-first|приват|данны/.test(q)) key = "local";
    const A = {
      rag: {
        ru: "RAG в Nexus работает в пять шагов: чанкинг заметок, эмбеддинг в векторный индекс, поиск top-k, сборка контекста и стриминг ответа со ссылками. Поиск опирается на HNSW-индекс.",
        en: "RAG in Nexus runs in five steps: chunk notes, embed into a vector index, retrieve top-k, assemble context, and stream the answer with citations. Retrieval is backed by an HNSW index.",
        sources: ["rag-pipeline", "embeddings", "nexus"],
      },
      embed: {
        ru: "Эмбеддинги — векторное представление текста, где близость по косинусу отражает смысловую близость. Они индексируются через HNSW и используются в RAG-конвейере для поиска.",
        en: "Embeddings are vector representations of text where cosine proximity reflects semantic similarity. They're indexed via HNSW and used by the RAG pipeline for retrieval.",
        sources: ["embeddings", "rag-pipeline", "p3"],
      },
      local: {
        ru: "Local-first означает, что данные живут на устройстве, а сеть — это улучшение, а не зависимость. Nexus следует этому принципу; конфликты решаются three-way merge.",
        en: "Local-first means data lives on the device and the network is an enhancement, not a dependency. Nexus follows this principle; conflicts are resolved with a three-way merge.",
        sources: ["local-first", "nexus"],
      },
      default: {
        ru: "В этом vault'е центральная идея — связывать атомарные заметки через [[wikilink]], а не складывать в папки. Граф связей раскрывает структуру, скрытую иерархией папок.",
        en: "The central idea in this vault is to link atomic notes via [[wikilinks]] rather than filing them into folders. The link graph reveals structure that folders hide.",
        sources: ["second-brain", "nexus"],
      },
    };
    const a = A[key];
    return { text: a[lang] || a.en, sources: a.sources };
  }
  window.mockAnswer = mockAnswer;

  // ----- suggestions for current note -----
  function mockSuggestions(noteId) {
    const map = {
      "nexus": [
        { id: "rag-pipeline", score: 0.91 }, { id: "embeddings", score: 0.78 }, { id: "second-brain", score: 0.64 },
      ],
      "rag-pipeline": [
        { id: "p2", score: 0.88 }, { id: "embeddings", score: 0.82 }, { id: "second-brain", score: 0.59 },
      ],
      "embeddings": [
        { id: "p1", score: 0.86 }, { id: "p3", score: 0.81 }, { id: "rag-pipeline", score: 0.7 },
      ],
    };
    return map[noteId] || [
      { id: "second-brain", score: 0.55 }, { id: "nexus", score: 0.48 },
    ];
  }
  window.mockSuggestions = mockSuggestions;
})();
