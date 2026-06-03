// conflict.jsx — three-way merge resolver for a sync/external-edit conflict.
(function () {
  const { useState, useRef, useMemo } = React;
  const Icon = window.Icon;

  const STR = {
    ru: {
      title: "Конфликт синхронизации", file: "RAG Pipeline.md",
      sub: "Заметка изменена и здесь, и на диске. Выберите версию для каждого блока.",
      local: "Локально", localFull: "Локально (вы)", remote: "На диске", remoteFull: "На диске (внешнее)",
      both: "Оба", localEdits: "правки здесь", remoteEdits: "правки на диске",
      conflicts: "Конфликты", conflict: "Конфликт", choose: "выбрать",
      bulkLocal: "Везде локальные", bulkRemote: "Везде с диска",
      progress: (a, b) => `Разрешено ${a} из ${b}`,
      cancel: "Отмена", save: "Сохранить объединённое", keepBoth: "Оба",
      pending: "не выбрано",
    },
    en: {
      title: "Sync conflict", file: "RAG Pipeline.md",
      sub: "This note changed here and on disk. Pick a version for each block.",
      local: "Local", localFull: "Local (you)", remote: "On disk", remoteFull: "On disk (incoming)",
      both: "Both", localEdits: "local edits", remoteEdits: "disk edits",
      conflicts: "Conflicts", conflict: "Conflict", choose: "choose",
      bulkLocal: "All local", bulkRemote: "All from disk",
      progress: (a, b) => `${a} of ${b} resolved`,
      cancel: "Cancel", save: "Save merged", keepBoth: "Both",
      pending: "unresolved",
    },
  };

  // segments: same (context) | conflict (local vs remote)
  const SEGMENTS = [
    { type: "same", lines: ["# RAG Pipeline", "", "Retrieval-Augmented Generation: достаём релевантные чанки и кладём в контекст модели.", ""] },
    { type: "conflict", id: "c1",
      local: ["## Шаги конвейера", "1. Чанкинг заметок (≈512 токенов, перекрытие 64)"],
      remote: ["## Pipeline", "1. Чанкинг заметок (512 токенов)"] },
    { type: "same", lines: ["2. Эмбеддинг → векторный индекс", "3. Поиск top-k по запросу", ""] },
    { type: "conflict", id: "c2",
      local: ["4. Реранкинг кросс-энкодером", "5. Сборка контекста + промпт", "6. Стрим ответа со ссылками"],
      remote: ["4. Сборка контекста + промпт", "5. Стрим ответа на источники"] },
    { type: "same", lines: ["", "Реализуется в [[Nexus]]."] },
    { type: "conflict", id: "c3",
      local: ["", "> Локальная модель по умолчанию, облако — опционально.", "", "#ai #rag #pipeline"],
      remote: ["", "#ai #rag"] },
  ];
  const CONFLICT_IDS = SEGMENTS.filter((s) => s.type === "conflict").map((s) => s.id);

  function ConflictResolver({ lang, onClose, onResolved }) {
    const t = STR[lang] || STR.en;
    const [res, setRes] = useState({}); // id -> 'local'|'remote'|'both'
    const [flash, setFlash] = useState(null);
    const scrollRef = useRef(null);
    const hunkRefs = useRef({});

    const resolvedCount = CONFLICT_IDS.filter((id) => res[id]).length;
    const allResolved = resolvedCount === CONFLICT_IDS.length;

    function pick(id, side) { setRes((r) => ({ ...r, [id]: side })); }
    function bulk(side) { const next = {}; CONFLICT_IDS.forEach((id) => (next[id] = side)); setRes(next); }
    function jump(id) {
      const el = hunkRefs.current[id], sc = scrollRef.current;
      if (el && sc) sc.scrollTop = el.offsetTop - 16;
      setFlash(id); setTimeout(() => setFlash((f) => (f === id ? null : f)), 1100);
    }
    function save() {
      const merged = [];
      SEGMENTS.forEach((s) => {
        if (s.type === "same") merged.push(...s.lines);
        else { const side = res[s.id]; if (side === "local") merged.push(...s.local);
          else if (side === "remote") merged.push(...s.remote); else merged.push(...s.local, ...s.remote); }
      });
      onResolved && onResolved(merged.join("\n"));
    }

    let cidx = 0;
    return React.createElement("div", { className: "cfl-scrim", onMouseDown: (e) => { if (e.target === e.currentTarget) onClose(); } },
      React.createElement("div", { className: "cfl-panel", role: "dialog", "aria-label": t.title },
        // header
        React.createElement("div", { className: "cfl-head" },
          React.createElement("div", { className: "cfl-ic" }, React.createElement(Icon, { name: "git-merge", size: 20 })),
          React.createElement("div", { className: "cfl-tt" },
            React.createElement("div", { className: "cfl-title" }, t.title, React.createElement("span", { className: "fname" }, t.file)),
            React.createElement("div", { className: "cfl-sub" }, t.sub)),
          React.createElement("button", { className: "tb-btn", onClick: onClose, "aria-label": "close" }, React.createElement(Icon, { name: "x", size: 16 }))),
        // doc
        React.createElement("div", { className: "cfl-doc", ref: scrollRef },
          SEGMENTS.map((s, i) => {
            if (s.type === "same")
              return React.createElement("div", { key: i, className: "cfl-same" },
                s.lines.map((ln, j) => ln === ""
                  ? React.createElement("div", { key: j, className: "cfl-gapline" })
                  : React.createElement("div", { key: j, className: "cfl-line" }, ln)));
            cidx++; const n = cidx; const choice = res[s.id];
            return React.createElement("div", {
              key: s.id, className: "cfl-hunk" + (flash === s.id ? " flash" : ""),
              ref: (el) => (hunkRefs.current[s.id] = el),
            },
              React.createElement("div", { className: "cfl-hunk-bar" },
                React.createElement(Icon, { name: "git-merge", size: 12 }),
                React.createElement("span", null, t.conflict, " ", React.createElement("span", { className: "num" }, n)),
                React.createElement("span", { style: { marginLeft: "auto", color: choice ? "var(--color-success)" : "var(--color-warning)" } },
                  choice ? (choice === "local" ? t.local : choice === "remote" ? t.remote : t.both) : t.pending)),
              ["local", "remote"].map((side) =>
                React.createElement("div", {
                  key: side, className: "cfl-side " + side + (choice === side ? " chosen" : "") + (choice && choice !== side && choice !== "both" ? " dimmed" : ""),
                  onClick: () => pick(s.id, side),
                },
                  React.createElement("span", { className: "cfl-tag" }, side === "local" ? "Л" : "Д"),
                  React.createElement("div", { className: "cfl-sidehead" },
                    React.createElement(Icon, { name: side === "local" ? "pin" : "drive", size: 12 }),
                    side === "local" ? t.localFull : t.remoteFull),
                  (side === "local" ? s.local : s.remote).join("\n"),
                  choice === side || choice === "both"
                    ? React.createElement("span", { className: "cfl-check" }, React.createElement(Icon, { name: "check", size: 12, strokeWidth: 2.5 })) : null)),
              React.createElement("div", { className: "cfl-hunk-actions" },
                React.createElement("button", { className: "cfl-pick" + (choice === "local" ? " on-local" : ""), onClick: () => pick(s.id, "local") }, React.createElement(Icon, { name: "pin", size: 13 }), t.local),
                React.createElement("button", { className: "cfl-pick" + (choice === "remote" ? " on-remote" : ""), onClick: () => pick(s.id, "remote") }, React.createElement(Icon, { name: "drive", size: 13 }), t.remote),
                React.createElement("button", { className: "cfl-pick" + (choice === "both" ? " on-both" : ""), onClick: () => pick(s.id, "both") }, t.keepBoth)));
          })),
        // rail
        React.createElement("div", { className: "cfl-rail" },
          React.createElement("div", { className: "cfl-rail-scroll" },
            React.createElement("div", { className: "cfl-stat" },
              React.createElement("div", { className: "box local" }, React.createElement("div", { className: "v" }, "4"), React.createElement("div", { className: "l" }, t.localEdits)),
              React.createElement("div", { className: "box remote" }, React.createElement("div", { className: "v" }, "2"), React.createElement("div", { className: "l" }, t.remoteEdits))),
            React.createElement("div", null,
              React.createElement("div", { className: "cfl-rail-h", style: { marginBottom: 8 } }, t.conflicts, " · ", resolvedCount, "/", CONFLICT_IDS.length),
              React.createElement("div", { style: { display: "flex", flexDirection: "column", gap: 2 } },
                CONFLICT_IDS.map((id, i) => {
                  const c = res[id];
                  return React.createElement("div", { key: id, className: "cfl-jump" + (c ? " resolved" : ""), onClick: () => jump(id) },
                    React.createElement("span", { className: "dot" }),
                    React.createElement("span", { className: "jt" }, t.conflict + " " + (i + 1)),
                    c ? React.createElement("span", { className: "jr " + c }, c === "local" ? t.local : c === "remote" ? t.remote : t.both) : null);
                }))),
            React.createElement("div", { className: "cfl-bulk" },
              React.createElement("div", { className: "cfl-rail-h", style: { marginBottom: 4 } }, t.choose),
              React.createElement("button", { onClick: () => bulk("local") }, React.createElement("span", { className: "sw local" }), t.bulkLocal),
              React.createElement("button", { onClick: () => bulk("remote") }, React.createElement("span", { className: "sw remote" }), t.bulkRemote)))),
        // footer
        React.createElement("div", { className: "cfl-foot" },
          React.createElement("div", { className: "cfl-progress" },
            React.createElement("div", { className: "bar" }, React.createElement("i", { style: { width: (resolvedCount / CONFLICT_IDS.length * 100) + "%" } })),
            t.progress(resolvedCount, CONFLICT_IDS.length)),
          React.createElement("button", { className: "btn btn-text", onClick: onClose }, t.cancel),
          React.createElement("button", { className: "btn btn-primary", disabled: !allResolved, onClick: save },
            React.createElement(Icon, { name: "check", size: 16 }), t.save))));
  }
  window.ConflictResolver = ConflictResolver;
})();
