// palette.jsx — Command Palette (Cmd+K). style: top | center | spotlight.
(function () {
  const { useState, useRef, useEffect, useMemo } = React;
  const Icon = window.Icon;

  function CommandPalette({ t, style, onClose, onOpenNote, commands }) {
    const [q, setQ] = useState("");
    const [sel, setSel] = useState(0);
    const inputRef = useRef(null);
    useEffect(() => { inputRef.current && inputRef.current.focus(); }, []);

    const files = useMemo(() => Object.entries(window.NEXUS_NOTES).map(([id, n]) => ({ id, title: n.title })), []);
    const lq = q.toLowerCase();

    const cmdMatches = q.startsWith(">") || !q
      ? commands.filter((c) => c.label.toLowerCase().includes(lq.replace(/^>/, "").trim()))
      : commands.filter((c) => c.label.toLowerCase().includes(lq)).slice(0, 4);
    const fileMatches = q.startsWith(">") ? [] : files.filter((f) => f.title.toLowerCase().includes(lq)).slice(0, 8);

    const flat = useMemo(() => {
      const arr = [];
      if (fileMatches.length) { arr.push({ section: t.files }); fileMatches.forEach((f) => arr.push({ type: "file", ...f })); }
      if (cmdMatches.length) { arr.push({ section: t.commands }); cmdMatches.forEach((c) => arr.push({ type: "cmd", ...c })); }
      return arr;
    }, [q]);
    const items = flat.filter((x) => !x.section);
    useEffect(() => { setSel(0); }, [q]);

    function run(item) {
      if (!item) return;
      if (item.type === "file") onOpenNote(item.id);
      else if (item.type === "cmd") item.run();
      onClose();
    }
    function onKey(e) {
      if (e.key === "Escape") { onClose(); return; }
      if (e.key === "ArrowDown") { e.preventDefault(); setSel((s) => Math.min(items.length - 1, s + 1)); }
      else if (e.key === "ArrowUp") { e.preventDefault(); setSel((s) => Math.max(0, s - 1)); }
      else if (e.key === "Enter") { e.preventDefault(); run(items[sel]); }
    }

    let idx = -1;
    return React.createElement("div", { className: "cmd-scrim " + style, onMouseDown: (e) => { if (e.target === e.currentTarget) onClose(); } },
      React.createElement("div", { className: "cmd-palette", role: "dialog", "aria-label": t.search_files_cmds },
        React.createElement("div", { className: "cmd-input-row" },
          React.createElement(Icon, { name: style === "spotlight" ? "command" : "search", size: style === "spotlight" ? 20 : 17 }),
          React.createElement("input", {
            ref: inputRef, className: "cmd-input", value: q, onKeyDown: onKey,
            onChange: (e) => setQ(e.target.value), placeholder: t.search_files_cmds,
          }),
          q ? React.createElement("button", { className: "tb-btn", onClick: () => setQ("") }, React.createElement(Icon, { name: "x", size: 14 })) : React.createElement("span", { className: "kbd" }, "Esc"),
        ),
        React.createElement("div", { className: "cmd-results" },
          flat.length === 0
            ? React.createElement("div", { className: "cmd-empty" }, t.no_results)
            : flat.map((row, i) => {
                if (row.section) return React.createElement("div", { key: "s" + i, className: "cmd-section" }, row.section);
                idx++; const myIdx = idx;
                return React.createElement("div", {
                  key: row.id || row.label, className: "cmd-item" + (myIdx === sel ? " sel" : ""),
                  style: { "--cmd-i": myIdx },
                  onMouseEnter: () => setSel(myIdx), onClick: () => run(row),
                },
                  React.createElement(Icon, { name: row.type === "file" ? "file-text" : (row.icon || "command"), size: 16 }),
                  React.createElement("span", { className: "ci-name" }, row.type === "file" ? row.title : row.label),
                  row.hint ? React.createElement("span", { className: "ci-hint kbd" }, row.hint) : null,
                );
              })
        ),
        React.createElement("div", { className: "cmd-foot" },
          React.createElement("span", { className: "kb-hint" }, React.createElement("span", { className: "kbd" }, "↑↓"), "навигация / navigate"),
          React.createElement("span", { className: "kb-hint" }, React.createElement("span", { className: "kbd" }, "↵"), "открыть / open"),
          React.createElement("span", { className: "kb-hint", style: { marginLeft: "auto" } }, React.createElement(Icon, { name: "command", size: 11 }), "Nexus"),
        ),
      ),
    );
  }
  window.CommandPalette = CommandPalette;
})();
