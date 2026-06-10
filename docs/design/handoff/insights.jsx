// insights.jsx — Digest, Goals, Contradictions AI modals (shared pattern).
(function () {
  const { useState } = React;
  const Icon = window.Icon;
  const Think = window.BrandThinking;

  // ── shared modal shell ──
  function Modal({ kind, icon, title, gen, refresh, onClose, children }) {
    return React.createElement("div", { className: "ins-scrim", onMouseDown: (e) => { if (e.target === e.currentTarget) onClose(); } },
      React.createElement("div", { className: "ins-panel " + kind, role: "dialog", "aria-label": title },
        React.createElement("div", { className: "ins-head" },
          React.createElement("div", { className: "ih-ic" }, React.createElement(Icon, { name: icon, size: 18 })),
          React.createElement("div", { className: "ih-title" }, title),
          gen ? React.createElement("button", { className: "ins-gen", onClick: gen.onClick, disabled: gen.busy },
            gen.busy ? React.createElement(Think, { size: 14 }) : React.createElement(Icon, { name: "sparkles", size: 14 }),
            gen.busy ? gen.busyLabel : gen.label) : null,
          refresh ? React.createElement("button", { className: "tb-btn", onClick: refresh, title: "↻" }, React.createElement(Icon, { name: "refresh", size: 15 })) : null,
          React.createElement("button", { className: "tb-btn", onClick: onClose, "aria-label": "close" }, React.createElement(Icon, { name: "x", size: 16 }))),
        React.createElement("div", { className: "ins-body" }, children)));
  }

  const L = {
    ru: {
      digest: "Дайджест изменений", generate: "Сгенерировать", generating: "Генерирую…",
      digestEmpty: "Дайджеста ещё нет", digestEmptySub: "Нажмите «Сгенерировать», чтобы получить сводку изменений в vault.",
      digestMeta: (d, n) => d + " · заметок: " + n,
      goals: "Цели", goalsEmpty: "Целей пока нет", goalsEmptySub: "Добавьте тег #goal и поле progress: 0–100 в заметку, чтобы отслеживать цель здесь.",
      noProgress: "нет прогресса",
      contra: "Поиск противоречий", find: "Найти", finding: "Ищу…",
      contraEmpty: "Противоречий не найдено", contraEmptySub: "Нажмите «Найти», чтобы проверить заметки на конфликтующие утверждения.",
      hard: "фактическое", temporal: "устарело", soft: "мягкое",
      thinking: "Анализирую vault…", searching: "Сверяю утверждения…",
    },
    en: {
      digest: "Change digest", generate: "Generate", generating: "Generating…",
      digestEmpty: "No digest yet", digestEmptySub: "Press “Generate” for a summary of recent changes in your vault.",
      digestMeta: (d, n) => d + " · notes: " + n,
      goals: "Goals", goalsEmpty: "No goals yet", goalsEmptySub: "Add a #goal tag and a progress: 0–100 field to a note to track it here.",
      noProgress: "no progress",
      contra: "Contradictions", find: "Find", finding: "Searching…",
      contraEmpty: "No contradictions found", contraEmptySub: "Press “Find” to check your notes for conflicting statements.",
      hard: "factual", temporal: "outdated", soft: "soft",
      thinking: "Analyzing vault…", searching: "Cross-checking claims…",
    },
  };

  // ───────── Digest ─────────
  function DigestPanel({ lang, onClose }) {
    const t = L[lang] || L.en;
    const [state, setState] = useState("empty"); // empty | loading | done
    const [digest, setDigest] = useState(null);
    function gen() {
      setState("loading");
      setTimeout(() => {
        setDigest({
          date: lang === "ru" ? "9 июня 2026, 14:20" : "Jun 9, 2026, 2:20 PM", notes: 12,
          body: lang === "ru"
            ? "За последние сутки в фокусе — **архитектура агентов**. Три новые заметки по KV-cache и офлоаду слоёв расширили ветку inference.\n\nЗаметка «**Local-First**» получила 4 новые обратные ссылки — становится узлом-хабом. «payment-service анализ» не трогали 32 дня — кандидат в архив.\n\nКонвейер RAG переписан: добавлен шаг реранкинга. Связано с целью «Агент анализа кода» (65%)."
            : "Over the last day, focus has been on **agent architecture**. Three new notes on KV-cache and layer offload expanded the inference branch.\n\n“**Local-First**” gained 4 new backlinks — becoming a hub. “payment-service analysis” untouched for 32 days — an archive candidate.\n\nThe RAG pipeline was rewritten with a reranking step. Tied to the “Code-analysis agent” goal (65%).",
        });
        setState("done");
      }, 1400);
    }
    return React.createElement(Modal, {
      kind: "digest", icon: "newspaper", title: t.digest, onClose,
      gen: { label: t.generate, busyLabel: t.generating, busy: state === "loading", onClick: gen },
      refresh: state === "done" ? gen : null,
    },
      state === "loading" ? React.createElement("div", { className: "ins-loading" },
        React.createElement(Think, { size: 30 }), React.createElement("span", { className: "mt-label" }, t.thinking)) :
      state === "empty" ? React.createElement("div", { className: "ins-empty" },
        React.createElement("div", { className: "ie-ic" }, React.createElement(Icon, { name: "newspaper", size: 22 })),
        React.createElement("div", { className: "ie-title" }, t.digestEmpty),
        React.createElement("div", { className: "ie-sub" }, t.digestEmptySub)) :
      React.createElement("div", null,
        React.createElement("div", { className: "dg-meta" }, t.digestMeta(digest.date, digest.notes), React.createElement("span", { className: "ai-badge" }, "AI")),
        React.createElement("div", { className: "dg-text", dangerouslySetInnerHTML: { __html: digest.body.replace(/\*\*(.+?)\*\*/g, "<strong>$1</strong>") } })));
  }

  // ───────── Goals ─────────
  const GOALS = [
    { id: "nexus", name: "Агент анализа кода", nameEn: "Code-analysis agent", progress: 65 },
    { id: "rag-pipeline", name: "YouTube-система", nameEn: "YouTube system", progress: 30 },
    { id: "embeddings", name: "GPU inference сетап", nameEn: "GPU inference setup", progress: 80 },
    { id: "local-first", name: "Книга заметок: PKM-метод", nameEn: "Notebook: PKM method", progress: null },
  ];
  function GoalsPanel({ lang, onClose, onOpenNote }) {
    const t = L[lang] || L.en;
    const [state, setState] = useState("loading");
    React.useEffect(() => { const id = setTimeout(() => setState("done"), 600); return () => clearTimeout(id); }, []);
    return React.createElement(Modal, { kind: "goals", icon: "target", title: t.goals, onClose, refresh: () => { setState("loading"); setTimeout(() => setState("done"), 600); } },
      state === "loading" ? React.createElement("div", { className: "ins-loading" }, React.createElement(Think, { size: 26 })) :
      GOALS.length === 0 ? React.createElement("div", { className: "ins-empty" },
        React.createElement("div", { className: "ie-ic" }, React.createElement(Icon, { name: "target", size: 22 })),
        React.createElement("div", { className: "ie-title" }, t.goalsEmpty),
        React.createElement("div", { className: "ie-sub" }, t.goalsEmptySub)) :
      React.createElement("div", null,
        GOALS.map((g) => React.createElement("div", { key: g.id, className: "goal-row" },
          React.createElement("span", { className: "goal-name", onClick: () => onOpenNote(g.id) }, lang === "ru" ? g.name : g.nameEn),
          g.progress == null
            ? React.createElement("span", { className: "goal-none" }, t.noProgress)
            : React.createElement("span", { className: "goal-prog" },
                React.createElement("span", { className: "goal-track" }, React.createElement("span", { className: "goal-fill", style: { width: g.progress + "%" } })),
                React.createElement("span", { className: "goal-pct" }, g.progress + "%"))))));
  }

  // ───────── Contradictions ─────────
  const CONTRA = [
    { a: "RAG Pipeline", aId: "rag-pipeline", b: "Embeddings", bId: "embeddings", type: "hard",
      why: { ru: "В одной заметке чанк — 512 токенов, в другой — 256. Зафиксируйте единый размер.", en: "One note says chunks are 512 tokens, another says 256. Pick one." } },
    { a: "Local-First", aId: "local-first", b: "Tauri Notes", bId: "tauri", type: "temporal",
      why: { ru: "Заметка о синхронизации ссылается на устаревший подход — переписана 6 мес назад.", en: "The sync note references an approach rewritten 6 months ago." } },
    { a: "Second Brain", aId: "second-brain", b: "Nexus", bId: "nexus", type: "soft",
      why: { ru: "Разный тон в определении «атомарной заметки» — скорее стилистика, чем факт.", en: "Different framing of “atomic note” — stylistic rather than factual." } },
  ];
  function ContraPanel({ lang, onClose, onOpenNote }) {
    const t = L[lang] || L.en;
    const [state, setState] = useState("empty");
    function find() { setState("loading"); setTimeout(() => setState("done"), 1500); }
    return React.createElement(Modal, {
      kind: "contra", icon: "scale", title: t.contra, onClose,
      gen: { label: t.find, busyLabel: t.finding, busy: state === "loading", onClick: find },
      refresh: state === "done" ? find : null,
    },
      state === "loading" ? React.createElement("div", { className: "ins-loading" },
        React.createElement(Think, { size: 30 }), React.createElement("span", { className: "mt-label" }, t.searching)) :
      state === "empty" ? React.createElement("div", { className: "ins-empty" },
        React.createElement("div", { className: "ie-ic" }, React.createElement(Icon, { name: "scale", size: 22 })),
        React.createElement("div", { className: "ie-title" }, t.contraEmpty),
        React.createElement("div", { className: "ie-sub" }, t.contraEmptySub)) :
      React.createElement("div", { className: "contra-list" },
        CONTRA.map((c, i) => React.createElement("div", { key: i, className: "contra-card" },
          React.createElement("div", { className: "contra-top" },
            React.createElement("div", { className: "contra-pair" },
              React.createElement("span", { className: "cn", onClick: () => onOpenNote(c.aId) }, c.a),
              React.createElement(Icon, { name: "git-merge", size: 13, className: "arrow" }),
              React.createElement("span", { className: "cn", onClick: () => onOpenNote(c.bId) }, c.b)),
            React.createElement("span", { className: "contra-badge " + c.type }, t[c.type])),
          React.createElement("div", { className: "contra-why" }, c.why[lang] || c.why.en)))));
  }

  window.DigestPanel = DigestPanel;
  window.GoalsPanel = GoalsPanel;
  window.ContraPanel = ContraPanel;
})();
