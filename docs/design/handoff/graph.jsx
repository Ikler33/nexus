// graph.jsx — force-directed link graph over real [[wikilinks]].
// Local N-hop (default) + Global, tag filters, drag/hover/click-to-open.
(function () {
  const { useState, useRef, useEffect, useMemo, useCallback } = React;
  const Icon = window.Icon;

  // tag → token color for node accents
  const TAG_COLOR = {
    ai: "var(--color-ai)", rag: "var(--color-ai)", paper: "var(--color-link)",
    project: "var(--color-accent)", "local-first": "var(--color-accent)",
    daily: "var(--color-tag)", method: "var(--color-tag)", pkm: "var(--color-tag)",
    philosophy: "var(--color-link)", dev: "var(--color-warning)", rust: "var(--color-warning)",
  };
  function nodeColor(tags) { for (const tg of tags || []) if (TAG_COLOR[tg]) return TAG_COLOR[tg]; return "var(--color-text-muted)"; }

  // build undirected graph from wikilinks
  function buildGraph() {
    const NOTES = window.NEXUS_NOTES, T2I = window.NEXUS_TITLE2ID;
    const re = /\[\[([^\]]+)\]\]/g;
    const nodes = {}, edgeSet = new Set(), edges = [];
    Object.entries(NOTES).forEach(([id, n]) => { nodes[id] = { id, title: n.title, tags: n.tags || [], deg: 0 }; });
    Object.entries(NOTES).forEach(([id, n]) => {
      let m; re.lastIndex = 0;
      while ((m = re.exec(n.body))) {
        const tgt = T2I[m[1].toLowerCase()];
        if (tgt && tgt !== id) {
          const key = [id, tgt].sort().join("|");
          if (!edgeSet.has(key)) { edgeSet.add(key); edges.push({ a: id, b: tgt }); nodes[id].deg++; nodes[tgt].deg++; }
        }
      }
    });
    const adj = {}; Object.keys(nodes).forEach((k) => (adj[k] = []));
    edges.forEach((e) => { adj[e.a].push(e.b); adj[e.b].push(e.a); });
    return { nodes, edges, adj };
  }

  function bfs(adj, start, depth) {
    const seen = { [start]: 0 }, q = [start];
    while (q.length) { const cur = q.shift(); if (seen[cur] >= depth) continue;
      (adj[cur] || []).forEach((nb) => { if (!(nb in seen)) { seen[nb] = seen[cur] + 1; q.push(nb); } }); }
    return seen;
  }

  // enrich the real graph with synthetic nodes for a rich GLOBAL view (Obsidian-like):
  // hub-and-spoke clusters + one dense clique + a halo of orphan notes
  function buildBig(G) {
    const nodes = {}, edges = [];
    Object.values(G.nodes).forEach((n) => (nodes[n.id] = { ...n, deg: 0 }));
    G.edges.forEach((e) => edges.push({ a: e.a, b: e.b }));
    let s = 99; const rnd = () => { s = (s * 1103515245 + 12345) & 0x7fffffff; return s / 0x7fffffff; };
    let k = 0; const tags = ["ai", "rag", "project", "paper", "daily", "dev", "philosophy", "method", "rust", "math"];
    const add = (tag, grp) => { const id = "_g" + (k++); nodes[id] = { id, title: "Заметка " + k, tags: tag ? [tag] : [], deg: 0, synthetic: true, grp: grp == null ? null : grp }; return id; };
    // hub-and-spoke clusters (the "stars") — each its own group so it clusters apart
    const hubs = [];
    for (let c = 0; c < 8; c++) {
      const tg = tags[c % tags.length], hub = add(tg, c); hubs.push(hub);
      const leaves = 5 + Math.floor(rnd() * 9);
      for (let i = 0; i < leaves; i++) edges.push({ a: hub, b: add(rnd() > 0.5 ? tg : null, c) });
    }
    for (let i = 0; i < hubs.length; i++) if (rnd() > 0.72) edges.push({ a: hubs[i], b: hubs[(i + 1) % hubs.length] });
    // dense clique (the tight blob) — its own group
    const clq = []; for (let i = 0; i < 9; i++) clq.push(add("dev", 8));
    for (let i = 0; i < clq.length; i++) for (let j = i + 1; j < clq.length; j++) if (rnd() > 0.4) edges.push({ a: clq[i], b: clq[j] });
    // a few short chains feeding the core — each its own group
    for (let c = 0; c < 3; c++) { let prev = add("paper", 9 + c); for (let i = 0; i < 5; i++) { const nx = add("paper", 9 + c); edges.push({ a: prev, b: nx }); prev = nx; } }
    // halo of orphan notes
    for (let i = 0; i < 95; i++) add(null);
    const adj = {}; Object.keys(nodes).forEach((id) => (adj[id] = []));
    edges.forEach((e) => { adj[e.a].push(e.b); adj[e.b].push(e.a); nodes[e.a].deg++; nodes[e.b].deg++; });
    return { nodes, edges, adj };
  }

  function GraphView({ t, lang, activeId, onOpen, onClose }) {
    const G = useMemo(buildGraph, []);
    const GBig = useMemo(() => buildBig(G), [G]);
    const [mode, setMode] = useState("local");
    const [depth, setDepth] = useState(2);
    const [tagFilter, setTagFilter] = useState(null);
    const [loading, setLoading] = useState(true);
    const [hover, setHover] = useState(null);
    const [dragId, setDragId] = useState(null);
    const [, force] = useState(0);
    const posRef = useRef({});
    const dragRef = useRef(null);
    const rafRef = useRef(null);
    const alphaRef = useRef(0);
    const visRef = useRef(null);
    const runningRef = useRef(false);
    const movedRef = useRef(false);
    const idleRef = useRef(0);
    const [orphanPop, setOrphanPop] = useState(null);
    const stageRef = useRef(null);
    // viewport: zoom + pan (Obsidian-style)
    const viewRef = useRef({ scale: 1, tx: 0, ty: 0 });
    const [settingsOpen, setSettingsOpen] = useState(false);
    const [params, setParams] = useState({ repel: 4200, linkDist: 62, center: 0.012, group: false });
    const paramsRef = useRef(params); paramsRef.current = params;
    const W = mode === "global" ? 1500 : 900, H = mode === "global" ? 1300 : 620;

    // visible subgraph
    const vis = useMemo(() => {
      let src, ids;
      if (mode === "local" && activeId && G.nodes[activeId]) {
        src = G; const seen = bfs(G.adj, activeId, depth); ids = new Set(Object.keys(seen));
      } else { src = GBig; ids = new Set(Object.keys(GBig.nodes)); }
      if (tagFilter) ids = new Set([...ids].filter((id) => (src.nodes[id].tags || []).includes(tagFilter) || id === activeId));
      const nodes = [...ids].map((id) => src.nodes[id]);
      const edges = src.edges.filter((e) => ids.has(e.a) && ids.has(e.b));
      return { nodes, edges, idset: ids };
    }, [mode, depth, tagFilter, activeId, G, GBig]);

    const neighborSet = useMemo(() => {
      const focus = dragId || hover;
      if (!focus) return null; const s = new Set([focus]);
      vis.edges.forEach((e) => { if (e.a === focus) s.add(e.b); if (e.b === focus) s.add(e.a); }); return s;
    }, [hover, dragId, vis]);

    // kin of the currently-open note (active node) — highlighted on open
    const activeKin = useMemo(() => {
      const s = new Set();
      if (!activeId) return s;
      vis.edges.forEach((e) => { if (e.a === activeId) s.add(e.b); if (e.b === activeId) s.add(e.a); });
      return s;
    }, [activeId, vis]);

    // keep latest visible set reachable from the persistent sim loop
    visRef.current = vis;

    // ── persistent, re-heatable force simulation ──
    function step() {
      const V = visRef.current; if (!V) return;
      const P = posRef.current, cx = W / 2, cy = H / 2;
      const pr = paramsRef.current;
      // alpha floor while dragging so springs keep pulling neighbours
      const a = dragRef.current ? Math.max(alphaRef.current, 0.45) : alphaRef.current;
      const ids = V.nodes.map((n) => n.id);
      // repulsion (orphans barely repel each other → loose halo, not flung to corners)
      for (let i = 0; i < ids.length; i++) for (let j = i + 1; j < ids.length; j++) {
        const A = P[ids[i]], B = P[ids[j]];
        let dx = A.x - B.x, dy = A.y - B.y, d2 = dx * dx + dy * dy || 0.01;
        let fct = 1; if (A.ring && B.ring) fct = 0.12; else if (A.ring || B.ring) fct = 0.5;
        const f = (pr.repel * fct / d2) * a; const d = Math.sqrt(d2);
        A.vx += (dx / d) * f; A.vy += (dy / d) * f; B.vx -= (dx / d) * f; B.vy -= (dy / d) * f;
      }
      // springs (edges pull connected nodes toward a rest length)
      V.edges.forEach((e) => {
        const A = P[e.a], B = P[e.b]; if (!A || !B) return;
        let dx = B.x - A.x, dy = B.y - A.y; const d = Math.sqrt(dx * dx + dy * dy) || 0.01;
        const f = (d - pr.linkDist) * 0.05 * a;
        A.vx += (dx / d) * f; A.vy += (dy / d) * f; B.vx -= (dx / d) * f; B.vy -= (dy / d) * f;
      });
      // grouping — same dominant tag attracts toward a shared centroid (Obsidian "groups")
      if (pr.group) {
        const groups = {};
        ids.forEach((id) => {
          const n = V.nodes.find((x) => x.id === id);
          const g = (n && n.tags && n.tags[0]) || "_";
          (groups[g] || (groups[g] = { x: 0, y: 0, n: 0 }));
          groups[g].x += P[id].x; groups[g].y += P[id].y; groups[g].n++;
        });
        Object.values(groups).forEach((gp) => { gp.x /= gp.n; gp.y /= gp.n; });
        V.nodes.forEach((n) => {
          if (dragRef.current === n.id) return;
          const g = (n.tags && n.tags[0]) || "_"; const gp = groups[g]; const N = P[n.id];
          N.vx += (gp.x - N.x) * 0.03 * a; N.vy += (gp.y - N.y) * 0.03 * a;
        });
      }
      // gravity / ring / cluster-anchor + integrate (dragged node pinned to cursor)
      const glob = !(mode === "local" && activeId && G.nodes[activeId]);
      ids.forEach((id) => {
        const N = P[id]; if (!N) return;
        if (dragRef.current === id) { N.vx = 0; N.vy = 0; return; }
        if (N.ring) {
          const ddx = N.x - cx, ddy = N.y - cy, d = Math.hypot(ddx, ddy) || 0.01;
          N.vx += (ddx / d) * (N.ring - d) * 0.08 * a; N.vy += (ddy / d) * (N.ring - d) * 0.08 * a;
        }
        else if (N.anchor) {
          const br = 1 + 0.035 * Math.sin(idleRef.current + (N.anchor.x + N.anchor.y) * 0.01);
          const tx = cx + (N.anchor.x - cx) * br, ty = cy + (N.anchor.y - cy) * br;
          N.vx += (tx - N.x) * 0.05 * a; N.vy += (ty - N.y) * 0.05 * a;
        }
        else { const cf = glob ? Math.max(pr.center, 0.022) : pr.center; N.vx += (cx - N.x) * cf * a; N.vy += (cy - N.y) * cf * a; }
        N.vx *= 0.85; N.vy *= 0.85; N.x += N.vx; N.y += N.vy;
        if (N.ring) {
          // hard radial clamp → orphans stay in the halo band
          const ddx = N.x - cx, ddy = N.y - cy, d = Math.hypot(ddx, ddy) || 0.01;
          const lo = N.ring * 0.78, hi = N.ring * 1.18;
          if (d < lo || d > hi) { const k = (d < lo ? lo : hi) / d; N.x = cx + ddx * k; N.y = cy + ddy * k; }
        } else if (glob) {
          // safety net: no connected node may exceed the core radius (never flung to corners)
          const ddx = N.x - cx, ddy = N.y - cy, d = Math.hypot(ddx, ddy) || 0.01, coreMax = Math.min(W, H) * 0.27;
          if (d > coreMax) { const k = coreMax / d; N.x = cx + ddx * k; N.y = cy + ddy * k; }
        }
        N.x = Math.max(20, Math.min(W - 20, N.x)); N.y = Math.max(20, Math.min(H - 20, N.y));
      });
      idleRef.current += 0.018;
      alphaRef.current *= 0.94;
      // never fully freeze — keep a gentle idle floor so the graph "breathes"
      if (!dragRef.current && alphaRef.current < 0.05) alphaRef.current = 0.05;
      force((v) => v + 1);
    }
    function loop() {
      step();
      rafRef.current = requestAnimationFrame(loop); // always running (idle breathing)
    }
    function kick(a) {
      alphaRef.current = Math.max(alphaRef.current, a);
      if (!runningRef.current) { runningRef.current = true; rafRef.current = requestAnimationFrame(loop); }
    }
    // expose to drag handlers
    const kickRef = useRef(kick); kickRef.current = kick;

    // (re)seed positions + heat the sim whenever the visible set changes
    useEffect(() => {
      const P = posRef.current; const cx = W / 2, cy = H / 2;
      const isGlobal = !(mode === "local" && activeId && G.nodes[activeId]);
      let s = 13; const rnd = () => { s = (s * 1103515245 + 12345) & 0x7fffffff; return s / 0x7fffffff; };
      // spread cluster anchors around an inner ring so the "stars" sit apart (Obsidian-like)
      const anchorR = Math.min(W, H) * 0.17, NG = 12;
      const grpAnchor = (g) => { const ang = (g / NG) * Math.PI * 2 + 0.6; return { x: cx + Math.cos(ang) * anchorR * (0.55 + (g % 3) * 0.22), y: cy + Math.sin(ang) * anchorR * (0.55 + (g % 3) * 0.22) }; };
      vis.nodes.forEach((n, i) => {
        if (!P[n.id]) {
          if (isGlobal && n.deg === 0) {
            // orphan halo — rough circle with jitter (not a perfect sphere)
            const ang = rnd() * Math.PI * 2, R = Math.min(W, H) * 0.42 * (0.8 + rnd() * 0.34);
            P[n.id] = { x: cx + Math.cos(ang) * R, y: cy + Math.sin(ang) * R, vx: 0, vy: 0, ring: R };
          } else if (isGlobal && n.grp != null) {
            const a0 = grpAnchor(n.grp);
            P[n.id] = { x: a0.x + (rnd() - 0.5) * 60, y: a0.y + (rnd() - 0.5) * 60, vx: 0, vy: 0, anchor: a0 };
          } else if (isGlobal) {
            // real connected notes (no group) — anchor them near the centre so they can't fly to corners
            const a0 = { x: cx + (rnd() - 0.5) * 120, y: cy + (rnd() - 0.5) * 120 };
            P[n.id] = { x: a0.x, y: a0.y, vx: 0, vy: 0, anchor: a0 };
          } else {
            const ang = (i / vis.nodes.length) * Math.PI * 2;
            P[n.id] = { x: cx + Math.cos(ang) * 120 + (rnd() - 0.5) * 50, y: cy + Math.sin(ang) * 120 + (rnd() - 0.5) * 50, vx: 0, vy: 0 };
          }
        } else if (!isGlobal) { if (P[n.id].ring) delete P[n.id].ring; if (P[n.id].anchor) delete P[n.id].anchor; }
        else if (isGlobal && !P[n.id].anchor && !P[n.id].ring) { P[n.id].anchor = n.grp != null ? grpAnchor(n.grp) : { x: cx + (rnd() - 0.5) * 120, y: cy + (rnd() - 0.5) * 120 }; }
      });
      setLoading(true);
      // restart the sim loop so it picks up the new closure (mode/W/H/glob), not the stale one
      cancelAnimationFrame(rafRef.current); runningRef.current = false;
      kick(1);
      const loadTimer = setTimeout(() => setLoading(false), 900);
      return () => { clearTimeout(loadTimer); };
    }, [vis]);

    // cleanup on unmount
    useEffect(() => () => { cancelAnimationFrame(rafRef.current); runningRef.current = false; }, []);

    // drag — pin node to cursor, neighbours follow via springs
    const svgRef = useRef(null);
    // client → world coords (undo viewport zoom/pan)
    const toLocal = (e) => {
      const r = svgRef.current.getBoundingClientRect();
      const cxp = e.touches ? e.touches[0].clientX : e.clientX, cyp = e.touches ? e.touches[0].clientY : e.clientY;
      const vbx = (cxp - r.left) / r.width * W, vby = (cyp - r.top) / r.height * H;
      const v = viewRef.current;
      return { x: (vbx - v.tx) / v.scale, y: (vby - v.ty) / v.scale };
    };
    // re-heat sim when forces change
    useEffect(() => { kickRef.current && kickRef.current(0.6); }, [params]);

    // ── zoom (wheel toward cursor) + pan (drag background) ──
    const zoomAt = (vbx, vby, factor) => {
      const v = viewRef.current;
      const ns = Math.max(0.25, Math.min(4, v.scale * factor));
      const wx = (vbx - v.tx) / v.scale, wy = (vby - v.ty) / v.scale;
      v.tx = vbx - wx * ns; v.ty = vby - wy * ns; v.scale = ns;
      force((x) => x + 1);
    };
    const onWheel = (e) => {
      e.preventDefault();
      const r = svgRef.current.getBoundingClientRect();
      const vbx = (e.clientX - r.left) / r.width * W, vby = (e.clientY - r.top) / r.height * H;
      zoomAt(vbx, vby, Math.exp(-e.deltaY * 0.0015));
    };
    const zoomBtn = (factor) => () => zoomAt(W / 2, H / 2, factor);
    const resetView = () => { viewRef.current = { scale: 1, tx: 0, ty: 0 }; kickRef.current(0.5); force((x) => x + 1); };
    const onStageDown = (e) => {
      if (e.target.closest && e.target.closest(".g-node")) return; // node has its own drag
      const v = viewRef.current, r = svgRef.current.getBoundingClientRect();
      const sx = e.clientX, sy = e.clientY, tx0 = v.tx, ty0 = v.ty;
      const move = (ev) => { v.tx = tx0 + (ev.clientX - sx) / r.width * W; v.ty = ty0 + (ev.clientY - sy) / r.height * H; force((x) => x + 1); };
      const up = () => { window.removeEventListener("mousemove", move); window.removeEventListener("mouseup", up); document.body.style.cursor = ""; };
      window.addEventListener("mousemove", move); window.addEventListener("mouseup", up); document.body.style.cursor = "grabbing";
    };

    const onDown = useCallback((id) => (e) => {
      e.preventDefault(); dragRef.current = id; setDragId(id); movedRef.current = false;
      const N0 = posRef.current[id];
      const start = toLocal(e); const off = N0 ? { x: N0.x - start.x, y: N0.y - start.y } : { x: 0, y: 0 };
      kickRef.current(0.7);
      const move = (ev) => {
        movedRef.current = true;
        const p = toLocal(ev); const N = posRef.current[id];
        if (N) { N.x = Math.max(40, Math.min(W - 40, p.x + off.x)); N.y = Math.max(36, Math.min(H - 36, p.y + off.y)); N.vx = N.vy = 0; }
        kickRef.current(0.5); // keep the sim warm so neighbours chase
      };
      const up = () => {
        dragRef.current = null; setDragId(null); kickRef.current(0.35);
        window.removeEventListener("mousemove", move); window.removeEventListener("mouseup", up);
        window.removeEventListener("touchmove", move); window.removeEventListener("touchend", up);
      };
      window.addEventListener("mousemove", move); window.addEventListener("mouseup", up);
      window.addEventListener("touchmove", move, { passive: false }); window.addEventListener("touchend", up);
    }, []);

    const allTags = window.NEXUS_TAGS.slice(0, 8);
    const P = posRef.current;

    return React.createElement("div", { className: "graph-view" },
      // toolbar
      React.createElement("div", { className: "graph-bar" },
        React.createElement("div", { className: "seg" },
          React.createElement("button", { className: "seg-btn" + (mode === "local" ? " on" : ""), onClick: () => setMode("local") }, t.graph_local),
          React.createElement("button", { className: "seg-btn" + (mode === "global" ? " on" : ""), onClick: () => setMode("global") }, t.graph_global)),
        mode === "local" ? React.createElement("label", { className: "graph-depth" },
          t.graph_depth,
          React.createElement("input", { type: "range", min: 1, max: 3, value: depth, onChange: (e) => setDepth(+e.target.value) }),
          React.createElement("span", { className: "mono" }, depth)) : null,
        React.createElement("div", { className: "graph-tags" },
          allTags.map((tg) => React.createElement("button", {
            key: tg.tag, className: "gt-chip" + (tagFilter === tg.tag ? " on" : ""),
            onClick: () => setTagFilter((f) => f === tg.tag ? null : tg.tag),
          }, "#" + tg.tag))),
        React.createElement("div", { className: "graph-spacer" }),
        React.createElement("div", { className: "graph-settings-wrap" },
          React.createElement("button", { className: "tb-btn" + (settingsOpen ? " active" : ""), onClick: () => setSettingsOpen((o) => !o), title: t.graph_forces || "Силы" },
            React.createElement(Icon, { name: "settings", size: 16 })),
          settingsOpen ? React.createElement("div", { className: "graph-settings" },
            React.createElement("div", { className: "gs-title" }, t.graph_forces || "Силы"),
            React.createElement("label", { className: "gs-row" },
              React.createElement("span", null, t.graph_repel || "Отталкивание"),
              React.createElement("input", { type: "range", min: 2500, max: 13000, step: 100, value: params.repel, onChange: (e) => setParams((p) => ({ ...p, repel: +e.target.value })) })),
            React.createElement("label", { className: "gs-row" },
              React.createElement("span", null, t.graph_linkdist || "Длина связей"),
              React.createElement("input", { type: "range", min: 50, max: 190, step: 2, value: params.linkDist, onChange: (e) => setParams((p) => ({ ...p, linkDist: +e.target.value })) })),
            React.createElement("label", { className: "gs-row" },
              React.createElement("span", null, t.graph_centerf || "Центр"),
              React.createElement("input", { type: "range", min: 0, max: 0.04, step: 0.002, value: params.center, onChange: (e) => setParams((p) => ({ ...p, center: +e.target.value })) })),
            React.createElement("label", { className: "gs-row gs-toggle", onClick: () => setParams((p) => ({ ...p, group: !p.group })) },
              React.createElement("span", null, t.graph_group || "Группировка по тегам"),
              React.createElement("span", { className: "gs-switch" + (params.group ? " on" : "") }, React.createElement("span", { className: "gs-knob" })))) : null),
        React.createElement("span", { className: "graph-stat mono" }, t.graph_stat(vis.nodes.length, vis.edges.length)),
        React.createElement("button", { className: "tb-btn", onClick: onClose, title: t.close }, React.createElement(Icon, { name: "x", size: 16 }))),
      mode === "global" ? React.createElement("div", { className: "graph-warn" },
        React.createElement(Icon, { name: "alert", size: 14 }), t.graph_global_warn) : null,
      // canvas
      React.createElement("div", { className: "graph-stage", ref: stageRef },
        loading ? React.createElement("div", { className: "graph-loading" },
          React.createElement(window.BrandThinking, { size: 26 }),
          React.createElement("span", { className: "mt-label" }, t.graph_loading)) : null,
        React.createElement("svg", { ref: svgRef, className: "graph-svg", viewBox: `0 0 ${W} ${H}`, preserveAspectRatio: "xMidYMid meet",
          onWheel, onMouseDown: onStageDown, style: { cursor: "grab" } },
          React.createElement("g", { className: "g-viewport", transform: `translate(${viewRef.current.tx} ${viewRef.current.ty}) scale(${viewRef.current.scale})` },
          (() => { let flowN = 0; return vis.edges.map((e, i) => {
            const A = P[e.a], B = P[e.b]; if (!A || !B) return null;
            const active = neighborSet ? (neighborSet.has(e.a) && neighborSet.has(e.b)) : true;
            const lit = dragId && (e.a === dragId || e.b === dragId);
            const flow = !dragId && activeId && (e.a === activeId || e.b === activeId);
            const fcls = flow ? " flow f" + ((flowN++ % 4) + 1) : "";
            return React.createElement("line", { key: i, x1: A.x, y1: A.y, x2: B.x, y2: B.y,
              className: "g-edge" + (active ? "" : " dim") + (lit ? " lit" : "") + fcls });
          }); })(),
          vis.nodes.map((n) => {
            const N = P[n.id]; if (!N) return null;
            const isActive = n.id === activeId;
            const r = Math.max(n.synthetic && n.deg === 0 ? 3.5 : 5.5, Math.min(15, 5 + n.deg * 1.6));
            const faded = neighborSet && !neighborSet.has(n.id);
            const isKin = !isActive && activeKin.has(n.id);
            // labels: hidden in the zoomed-out overview AND at deep zoom — visible only mid-zoom (Obsidian-like). Synthetic demo nodes never labelled.
            const vScale = viewRef.current.scale;
            const pin = isActive || n.id === hover || n.id === dragId;
            const labelOn = pin || (!n.synthetic && vScale >= 1.25 && vScale <= 3.2);
            return React.createElement("g", {
              key: n.id, className: "g-node" + (faded ? " faded" : "") + (isActive ? " active" : "") + (isKin ? " kin" : "") + (dragId === n.id ? " grabbing" : ""),
              "data-deg": n.deg,
              transform: `translate(${N.x},${N.y})`,
              onMouseEnter: () => setHover(n.id), onMouseLeave: () => setHover(null),
              onMouseDown: onDown(n.id), onTouchStart: onDown(n.id),
              onClick: (e) => {
                if (movedRef.current) { movedRef.current = false; return; }
                if (n.deg === 0) {
                  const sr = stageRef.current.getBoundingClientRect();
                  setOrphanPop({ id: n.id, sx: e.clientX - sr.left, sy: e.clientY - sr.top, phase: "info" });
                  return;
                }
                if (!window.NEXUS_NOTES[n.id]) return; onOpen(n.id);
              },
            },
              isActive ? React.createElement("circle", { r: r + 6, className: "g-pulse" }) : null,
              isActive ? React.createElement("circle", { r: r + 6, className: "g-ripple" }) : null,
              React.createElement("circle", { r, className: "g-dot", style: { fill: isActive ? "var(--color-accent)" : nodeColor(n.tags) } }),
              isActive ? React.createElement("circle", { r: r + 5, className: "g-ring" }) : null,
              isKin ? React.createElement("circle", { r: r + 3.5, className: "g-kinring" }) : null,
              labelOn ? React.createElement("text", { y: r + 14, className: "g-label", textAnchor: "middle" }, n.title) : null);
          }))),
        // zoom controls
        React.createElement("div", { className: "graph-zoom" },
          React.createElement("button", { className: "gz-btn", onClick: zoomBtn(1 / 1.3), title: "Отдалить" }, React.createElement(Icon, { name: "minus", size: 15 })),
          React.createElement("button", { className: "gz-btn gz-fit", onClick: resetView, title: "Сбросить вид" }, React.createElement(Icon, { name: "shrink", size: 14 })),
          React.createElement("button", { className: "gz-btn", onClick: zoomBtn(1.3), title: "Приблизить" }, React.createElement(Icon, { name: "plus", size: 15 }))),
        // orphan note popover — "why is this note isolated" + AI link suggestion
        orphanPop ? React.createElement("div", {
          className: "orphan-pop", style: { left: orphanPop.sx, top: orphanPop.sy },
          onMouseDown: (e) => e.stopPropagation(),
        },
          React.createElement("button", { className: "op-close", onClick: () => setOrphanPop(null) }, React.createElement(Icon, { name: "x", size: 13 })),
          React.createElement("div", { className: "op-head" },
            React.createElement("span", { className: "op-dot" }),
            React.createElement("span", null, lang === "ru" ? "Изолированная заметка" : "Isolated note")),
          React.createElement("div", { className: "op-sub" }, lang === "ru" ? "Нет обратных ссылок — не связана с остальным графом." : "No backlinks — not connected to the rest of the graph."),
          orphanPop.phase === "info" ? React.createElement("button", { className: "op-ai", onClick: () => {
            setOrphanPop((p) => ({ ...p, phase: "thinking" }));
            setTimeout(() => setOrphanPop((p) => p && p.phase === "thinking" ? { ...p, phase: "done", pick: ["RAG Pipeline", "Embeddings", "Second Brain"][Math.floor(Math.random() * 3)] } : p), 1100);
          } },
            React.createElement(window.BrandThinking, { size: 15 }),
            React.createElement("span", null, lang === "ru" ? "Предложить связь" : "Suggest a link")) : null,
          orphanPop.phase === "thinking" ? React.createElement("div", { className: "op-think" },
            React.createElement(window.BrandThinking, { size: 16 }),
            React.createElement("span", { className: "mt-label" }, lang === "ru" ? "Ищу связи…" : "Finding links…")) : null,
          orphanPop.phase === "done" ? React.createElement("div", { className: "op-result" },
            React.createElement("div", { className: "op-rlabel" }, lang === "ru" ? "Возможная связь:" : "Possible link:"),
            React.createElement("div", { className: "op-link", onClick: () => { setOrphanPop(null); } },
              React.createElement(Icon, { name: "link", size: 13 }),
              React.createElement("span", null, "[[" + orphanPop.pick + "]]"))) : null,
        ) : null,
      ),
    );
  }
  window.GraphView = GraphView;
})();
