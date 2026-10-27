// Interactive figures of the concept pages: charts drawn as SVG in the
// taktsim look (the same classes as the static figures from
// docs/concepts/scripts/svgkit.py), sliders, and one widget per kind of
// figure. A figure marked <figure class="ts-widget" data-widget="…"> keeps
// its static SVG until this script has drawn the live one, so every page
// still teaches without JavaScript and in print.
(function () {
  "use strict";
  const S = window.TaktsimSim;
  const W = 640;
  const SHIFT = 8 * 3600;
  let uidCounter = 0;

  // ------------------------------------------------------------ helpers

  const esc = (s) => String(s).replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]);
  const n1 = (x) => (Math.round(x * 10) / 10).toString();
  const fmt = (x, d) => (Number.isFinite(x) ? x.toFixed(d) : "–");

  function niceTicks(lo, hi, count) {
    const raw = (hi - lo) / (count || 5);
    const mag = Math.pow(10, Math.floor(Math.log10(raw)));
    const step = [1, 2, 2.5, 5, 10].map((m) => m * mag).find((s) => s >= raw) || 10 * mag;
    const out = [];
    for (let v = Math.ceil(lo / step - 1e-9) * step; v <= hi + 1e-9; v += step) out.push(Math.round(v * 1e10) / 1e10);
    return out;
  }

  class Svg {
    constructor(height, title) {
      this.uid = "w" + ++uidCounter;
      this.height = height;
      this.title = title;
      this.parts = [];
    }
    add(s) { this.parts.push(s); }
    rect(x, y, w, h, cls, fill) {
      this.add(`<rect x="${n1(x)}" y="${n1(y)}" width="${n1(w)}" height="${n1(h)}"${cls ? ` class="${cls}"` : ""}${fill ? ` fill="${fill}"` : ""}/>`);
    }
    line(x1, y1, x2, y2, cls) {
      this.add(`<line x1="${n1(x1)}" y1="${n1(y1)}" x2="${n1(x2)}" y2="${n1(y2)}" class="${cls}"/>`);
    }
    path(d, cls) { this.add(`<path d="${d}" class="${cls}"/>`); }
    text(x, y, s, anchor, cls) {
      this.add(`<text x="${n1(x)}" y="${n1(y)}" text-anchor="${anchor || "start"}"${cls ? ` class="${cls}"` : ""}>${esc(s)}</text>`);
    }
    stateRect(x, y, w, h, state) {
      if (state === "working") this.rect(x, y, w, h, "st-working");
      else if (state === "starved") this.rect(x + 0.5, y + 0.5, w - 1, h - 1, "st-starved");
      else if (state === "blocked") this.rect(x, y, w, h, "", `url(#${this.uid}-hatch)`);
      else if (state === "down") this.rect(x, y, w, h, "st-down");
      else this.rect(x, y, w, h, "", `url(#${this.uid}-dots)`);
    }
    render() {
      const u = this.uid;
      return `<svg xmlns="http://www.w3.org/2000/svg" class="ts-svg" viewBox="0 0 ${W} ${n1(this.height)}" role="img" aria-label="${esc(this.title)}">` +
        `<defs><pattern id="${u}-hatch" patternUnits="userSpaceOnUse" width="5" height="5" patternTransform="rotate(45)"><rect width="5" height="5" class="st-blocked-bg"/><line x1="0" y1="0" x2="0" y2="5" class="st-pattern-hatch"/></pattern>` +
        `<pattern id="${u}-dots" patternUnits="userSpaceOnUse" width="4" height="4"><rect width="4" height="4" class="st-blocked-bg"/><rect x="1" y="1" width="2" height="2" class="st-pattern-dots"/></pattern></defs>` +
        this.parts.join("") + "</svg>";
    }
  }

  // ------------------------------------------------------------- charts

  // series: [{label, xs, ys, ci, style: "ink" | "ref"}]; highlight: {x, y, label}
  function xyChart(o) {
    const height = o.height || 300;
    const svg = new Svg(height, o.title);
    const left = 64, right = 24, top = 20, bottom = 48;
    const sx = (v) => left + ((v - o.xr[0]) / (o.xr[1] - o.xr[0])) * (W - right - left);
    const sy = (v) => height - bottom - ((v - o.yr[0]) / (o.yr[1] - o.yr[0])) * (height - bottom - top);
    const xt = (v) => (o.xfmt ? o.xfmt(v) : String(v));
    for (const t of o.yticks || niceTicks(o.yr[0], o.yr[1])) {
      svg.line(left, sy(t), W - right, sy(t), "ts-grid");
      svg.text(left - 8, sy(t) + 4, String(t), "end");
    }
    for (const t of o.xticks || niceTicks(o.xr[0], o.xr[1])) {
      svg.line(sx(t), height - bottom, sx(t), height - bottom + 4, "ts-axis");
      svg.text(sx(t), height - bottom + 18, xt(t), "middle");
    }
    svg.line(left, height - bottom, W - right, height - bottom, "ts-axis");
    svg.line(left, top, left, height - bottom, "ts-axis");
    svg.text((left + W - right) / 2, height - 8, o.xlabel.toUpperCase(), "middle");
    const my = (top + height - bottom) / 2;
    svg.add(`<text x="14" y="${n1(my)}" text-anchor="middle" transform="rotate(-90 14 ${n1(my)})">${esc(o.ylabel.toUpperCase())}</text>`);
    for (const [v, label] of o.hlines || []) {
      svg.line(left, sy(v), W - right, sy(v), "ts-muted-line");
      svg.text(W - right - 4, sy(v) - 6, label, "end");
    }
    for (const s of o.series) {
      const inside = (y) => Number.isFinite(y) && y >= o.yr[0] && y <= o.yr[1];
      if (s.ci) {
        const up = s.xs.map((x, i) => [sx(x), sy(Math.min(s.ys[i] + s.ci[i], o.yr[1]))]);
        const lo = s.xs.map((x, i) => [sx(x), sy(Math.max(s.ys[i] - s.ci[i], o.yr[0]))]).reverse();
        svg.path("M" + up.concat(lo).map((p) => `${n1(p[0])} ${n1(p[1])}`).join(" L") + " Z", "ts-ci");
      }
      const pts = s.xs.map((x, i) => [sx(x), sy(s.ys[i]), s.ys[i]]).filter((p) => inside(p[2]));
      if (pts.length > 1) svg.path("M" + pts.map((p) => `${n1(p[0])} ${n1(p[1])}`).join(" L"), s.style === "ref" ? "ts-muted-line" : "ts-ink-line");
      if (s.style !== "ref" && s.markers !== false) for (const p of pts) svg.rect(p[0] - 3, p[1] - 3, 6, 6, "ts-ink-fill");
    }
    if (o.highlight && Number.isFinite(o.highlight.y)) {
      const x = sx(o.highlight.x), y = sy(Math.min(Math.max(o.highlight.y, o.yr[0]), o.yr[1]));
      if (o.highlight.ci) svg.rect(x - 1, sy(Math.min(o.highlight.y + o.highlight.ci, o.yr[1])), 2, Math.max(0, sy(Math.max(o.highlight.y - o.highlight.ci, o.yr[0])) - sy(Math.min(o.highlight.y + o.highlight.ci, o.yr[1]))), "ts-ink-fill");
      svg.rect(x - 6, y - 6, 12, 12, "ts-acid");
      const end = x > W * 0.7;
      svg.text(x + (end ? -12 : 12), y + (o.labelBelow ? 22 : -10), o.highlight.label, end ? "end" : "start", "ts-strong");
    }
    let lx = left + 12;
    for (const s of o.series.filter((s) => s.label)) {
      if (s.style === "ref") svg.line(lx, top + 7, lx + 14, top + 7, "ts-muted-line");
      else svg.rect(lx, top + 2, 10, 10, "ts-ink-fill");
      svg.text(lx + 18, top + 11, s.label.toUpperCase());
      lx += 30 + 8 * s.label.length;
    }
    return svg.render();
  }

  function dominant(trace, a, b) {
    const time = {};
    for (let i = 0; i < trace.length; i++) {
      const end = i + 1 < trace.length ? trace[i + 1][0] : Infinity;
      const lo = Math.max(trace[i][0], a), hi = Math.min(end, b);
      if (hi > lo) time[trace[i][1]] = (time[trace[i][1]] || 0) + hi - lo;
    }
    const order = ["down", "setup", "blocked", "starved", "working"];
    let best = order[0];
    for (const s of order) if ((time[s] || 0) > (time[best] || 0)) best = s;
    return best;
  }

  function meanLevel(trace, a, b) {
    let area = 0;
    for (let i = 0; i < trace.length; i++) {
      const end = i + 1 < trace.length ? trace[i + 1][0] : Infinity;
      const lo = Math.max(trace[i][0], a), hi = Math.min(end, b);
      if (hi > lo) area += trace[i][1] * (hi - lo);
    }
    return area / (b - a);
  }

  // rows: [[name, stateTrace]]; levels: [[name, levelTrace, capacity]]; mark: {t, label}
  function timeline(o) {
    const cells = o.cells || 60;
    const left = 92, right = 16, top = 52;
    const pitch = (W - left - right) / cells, cell = pitch - 2, rowH = cell + 12, lvlH = 54;
    const height = top + o.rows.length * rowH + (o.levels || []).length * (lvlH + 16) + 40;
    const svg = new Svg(height, o.title);
    const slice = (o.t1 - o.t0) / cells;
    let lx = left;
    for (const st of S.STATES) {
      if (o.rows.some((r) => r[1].some((e) => e[1] === st))) {
        svg.stateRect(lx, 8, 10, 10, st);
        svg.text(lx + 16, 17, st.toUpperCase());
        lx += 24 + 8 * st.length + 12;
      }
    }
    let y = top;
    for (const [name, tr] of o.rows) {
      svg.text(left - 10, y + cell * 0.85, name.toUpperCase(), "end", "ts-strong");
      for (let i = 0; i < cells; i++) svg.stateRect(left + i * pitch, y, cell, cell, dominant(tr, o.t0 + i * slice, o.t0 + (i + 1) * slice));
      y += rowH;
    }
    for (const [name, tr, cap] of o.levels || []) {
      y += 16;
      const base = y + lvlH;
      svg.text(left - 10, y + 10, name.toUpperCase(), "end", "ts-strong");
      svg.text(left - 10, y + 24, `max ${cap}`, "end");
      svg.line(left, base, W - right, base, "ts-axis");
      svg.line(left, y, W - right, y, "ts-grid");
      for (let i = 0; i < cells; i++) {
        const v = meanLevel(tr, o.t0 + i * slice, o.t0 + (i + 1) * slice);
        const h = cap ? (v / cap) * lvlH : 0;
        if (h > 0.5) svg.rect(left + i * pitch, base - h, cell, h, "ts-muted-fill");
      }
      y += lvlH;
    }
    if (o.mark) {
      const i = Math.min(cells - 1, Math.floor((o.mark.t - o.t0) / slice));
      const x = left + i * pitch;
      svg.rect(x, top - 10, cell, 6, "ts-acid");
      svg.line(x + cell / 2, top - 4, x + cell / 2, y, "ts-acid-line");
      if (x < W * 0.6) svg.text(x + cell + 6, top - 4, o.mark.label, "start", "ts-strong");
      else svg.text(x - 6, top - 4, o.mark.label, "end", "ts-strong");
    }
    const ty = y + 4;
    for (const f of [0, 0.25, 0.5, 0.75, 1]) {
      const x = left + f * (W - left - right);
      svg.text(x, ty + 14, `${n1((o.t0 + f * (o.t1 - o.t0)) / 60)} min`, f === 0 ? "start" : f === 1 ? "end" : "middle");
    }
    return svg.render();
  }

  // ------------------------------------------------------------ widgets

  // Builds the controls; `params` maps a key to {label, min, max, step, value, format}.
  function controls(fig, params, onChange) {
    const box = document.createElement("div");
    box.className = "ts-controls";
    const values = {};
    for (const [key, p] of Object.entries(params)) {
      values[key] = p.value;
      const id = `ts-${++uidCounter}-${key}`;
      const div = document.createElement("div");
      div.className = "ts-control";
      div.innerHTML = `<label for="${id}"><span>${esc(p.label)}</span><output for="${id}"></output></label>` +
        `<input type="range" id="${id}" min="${p.min}" max="${p.max}" step="${p.step}" value="${p.value}">`;
      const input = div.querySelector("input"), out = div.querySelector("output");
      const show = () => { out.textContent = p.format ? p.format(values[key]) : String(values[key]); };
      show();
      let timer = null;
      input.addEventListener("input", () => {
        values[key] = Number(input.value);
        show();
        clearTimeout(timer);
        timer = setTimeout(() => onChange(values), 60);
      });
      box.appendChild(div);
    }
    return { box, values };
  }

  function readout(html) {
    const div = document.createElement("div");
    div.className = "ts-readout";
    div.setAttribute("aria-live", "polite");
    div.innerHTML = html;
    return div;
  }

  const lineA = (sigma, buffer, extra) => ({
    stations: [
      S.station("cut", S.Dist.lognormal(42, sigma), extra && extra.cut),
      S.station("weld", S.Dist.lognormal(45, sigma), extra && extra.weld),
    ],
    buffers: [buffer],
  });

  const WIDGETS = {};

  // Page 5: one station with random arrivals; wait in queue against utilization.
  WIDGETS.utilization = (live, cfg) => {
    const t = cfg.process_mean || 45;
    const plot = document.createElement("div");
    plot.className = "ts-plot";
    const info = readout("");
    const kingman = (u, cv) => ((1 + cv * cv) / 2) * (u / (1 - u)) * t / 60;
    const draw = (v) => {
      const u = v.u, cv = v.cv;
      const proc = cv === 1 ? S.Dist.exp(t) : S.Dist.lognormalCv(t, cv);
      const line = { stations: [S.station("weld", proc)], buffers: [], arrivals: S.Dist.exp(t / u) };
      const runs = S.replicate(line, 400000, 8, 20000).map((r) => r.queueWait / 60);
      const e = S.estimate(runs);
      const grid = [];
      for (let x = 0.3; x <= 0.975; x += 0.005) grid.push(x);
      plot.innerHTML = xyChart({
        title: "Wait in queue against utilization",
        series: [{ label: "Kingman", xs: grid, ys: grid.map((x) => kingman(x, cv)), style: "ref" }],
        xlabel: "utilization", ylabel: "wait in queue (min)", xr: [0.3, 1], yr: [0, 16],
        xticks: [0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9, 1],
        highlight: { x: u, y: e.mean, ci: e.half, label: `simulated: ${fmt(e.mean, 1)} min` },
      });
      info.innerHTML = `<span>simulated <b>${fmt(e.mean, 2)} ± ${fmt(e.half, 2)} min</b></span>` +
        `<span>Kingman <b>${fmt(kingman(u, cv), 2)} min</b></span><span>8 runs of 4.6 days</span>`;
    };
    const c = controls(live, {
      u: { label: "Utilization", min: 0.5, max: 0.97, step: 0.01, value: cfg.u || 0.85, format: (x) => `${Math.round(x * 100)} %` },
      cv: { label: "Process time CV", min: 0, max: 1.5, step: 0.05, value: cfg.cv == null ? 1 : cfg.cv, format: (x) => x.toFixed(2) },
    }, draw);
    live.append(c.box, plot, info);
    draw(c.values);
  };

  // Page 6: state timeline of line A with a small buffer.
  WIDGETS.timeline = (live, cfg) => {
    const plot = document.createElement("div");
    plot.className = "ts-plot";
    const info = readout("");
    let seed = cfg.seed || 2;
    const window_ = 30 * 60;
    const draw = (v) => {
      const r = S.simulate(lineA(v.sigma, v.buffer), SHIFT, seed, 0, true);
      const step = window_ / 60;
      let mark = null;
      for (let i = 0; i < 60; i++) {
        if (i * step > 300 && dominant(r.states[1], i * step, (i + 1) * step) === "starved") {
          mark = { t: i * step, label: "weld starved: buffer empty" };
          break;
        }
      }
      plot.innerHTML = timeline({
        title: "State timeline of cutting and welding",
        rows: [["cut", r.states[0]], ["weld", r.states[1]]],
        t0: 0, t1: window_, levels: [["buffer", r.levels[0], v.buffer]], mark,
      });
      const pct = (x) => `${Math.round(x * 100)} %`;
      info.innerHTML = `<span>whole shift: cut blocked <b>${pct(r.share(0, "blocked"))}</b></span>` +
        `<span>weld starved <b>${pct(r.share(1, "starved"))}</b></span><span>parts <b>${r.completed}</b></span><span>run ${seed}</span>`;
    };
    const c = controls(live, {
      sigma: { label: "Sigma, both stations", min: 0, max: 1, step: 0.05, value: cfg.sigma == null ? 0.5 : cfg.sigma, format: (x) => x.toFixed(2) },
      buffer: { label: "Buffer places", min: 0, max: 5, step: 1, value: cfg.buffer == null ? 1 : cfg.buffer },
    }, draw);
    const again = document.createElement("button");
    again.className = "ts-button";
    again.type = "button";
    again.textContent = "New run";
    again.addEventListener("click", () => { seed += 1; draw(c.values); });
    c.box.appendChild(again);
    live.append(c.box, plot, info);
    draw(c.values);
  };

  // Page 7: parts per shift against buffer size.
  WIDGETS.buffers = (live, cfg) => {
    const sizes = [0, 1, 2, 3, 5, 8, 10, 15, 20, 30];
    const plot = document.createElement("div");
    plot.className = "ts-plot";
    const info = readout("");
    const cache = {};
    const curve = (sigma) => {
      if (!cache[sigma]) {
        const ests = sizes.map((b) => S.estimate(S.replicate(lineA(sigma, b), SHIFT, 10).map((r) => r.completed)));
        cache[sigma] = { ys: ests.map((e) => e.mean), ci: ests.map((e) => e.half) };
      }
      return cache[sigma];
    };
    // The README's line: cutting lognormal(42 s, 0.08), welding a constant 45 s.
    const readme = (b) => ({
      stations: [S.station("cut", S.Dist.lognormal(42, 0.08)), S.station("weld", S.Dist.const(45))],
      buffers: [b],
    });
    const readmeYs = sizes.map((b) => S.estimate(S.replicate(readme(b), SHIFT, 10).map((r) => r.completed)).mean);
    const draw = (v) => {
      const cv = curve(v.sigma);
      const e = S.estimate(S.replicate(lineA(v.sigma, v.buffer), SHIFT, 30).map((r) => r.completed));
      const low = Math.floor((Math.min(...cv.ys, e.mean) - 30) / 50) * 50;
      plot.innerHTML = xyChart({
        title: "Parts per shift against buffer size",
        series: [
          { label: `sigma ${v.sigma.toFixed(2)}`, xs: sizes, ys: cv.ys, ci: cv.ci },
          { label: "readme line, cv 0.08", xs: sizes, ys: readmeYs, style: "ref" },
        ],
        xlabel: "buffer places", ylabel: "parts per shift", xr: [0, 30], yr: [Math.max(0, low), 660],
        highlight: { x: v.buffer, y: e.mean, ci: e.half, label: `${v.buffer} places: ${Math.round(e.mean)}` }, labelBelow: true,
      });
      info.innerHTML = `<span>${v.buffer} places: <b>${fmt(e.mean, 1)} ± ${fmt(e.half, 1)}</b> parts per shift</span><span>30 runs</span>`;
    };
    const c = controls(live, {
      buffer: { label: "Buffer places", min: 0, max: 30, step: 1, value: cfg.buffer == null ? 5 : cfg.buffer },
      sigma: { label: "Sigma, both stations", min: 0.05, max: 1, step: 0.05, value: cfg.sigma == null ? 0.5 : cfg.sigma, format: (x) => x.toFixed(2) },
    }, draw);
    live.append(c.box, plot, info);
    draw(c.values);
  };

  function mount(fig) {
    const make = WIDGETS[fig.dataset.widget];
    const live = fig.querySelector(".ts-live");
    if (!make || !live || !S) return;
    let cfg = {};
    try { cfg = JSON.parse(fig.dataset.config || "{}"); } catch (e) { cfg = {}; }
    make(live, cfg);
    fig.classList.add("ts-ready");
  }

  window.TaktsimViz = { xyChart, timeline, WIDGETS, mount };
  const start = () => document.querySelectorAll("figure.ts-widget").forEach(mount);
  if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", start);
  else start();
})();
