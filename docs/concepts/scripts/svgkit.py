"""SVG figures in the taktsim look, for the concept pages.

The figures are inlined into the pages, so they take their colours and
fonts from the site's stylesheet (website/theme/taktsim.css) and follow the
light and dark theme. The <style> block in each file only supplies
fallbacks from website/theme/tokens.json, for viewing a file on its own.

Rules: square marks, hairline axes, mono labels, and at most one acid mark
per figure, on the thing the figure is about.
"""

from __future__ import annotations

import json
import math
from collections.abc import Iterable, Sequence
from dataclasses import dataclass, field
from html import escape
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
TOKENS = json.loads((ROOT / "website" / "theme" / "tokens.json").read_text(encoding="utf-8"))

W = 640  # viewBox width of every figure


def tokens_css() -> str:
    """website/theme/tokens.css: the tokens as custom properties per theme."""
    col = {k: v for k, v in TOKENS["color"].items() if not k.startswith("_")}

    def block(theme: str) -> str:
        lines = [f"  --ts-{name}: {v[theme]};" for name, v in col.items()]
        lines.append(f"  --ts-focus: {'var(--ts-acid)' if theme == 'dark' else 'var(--ts-ink)'};")
        return "\n".join(lines)

    font = TOKENS["font"]
    return (
        "/* Generated from tokens.json by docs/concepts/scripts/build_all.py; do not edit. */\n\n"
        f":root {{\n  --ts-mono: {font['mono']};\n  --ts-sans: {font['sans']};\n}}\n\n"
        f".light, .rust, html:not(.js) {{\n{block('light')}\n}}\n\n"
        f".coal, .navy, .ayu {{\n{block('dark')}\n}}\n\n"
        f"@media (prefers-color-scheme: dark) {{\n  html:not(.js) {{\n{block('dark')}\n  }}\n}}\n"
    )


def _fallback_style() -> str:
    c = {k: v["dark"] for k, v in TOKENS["color"].items() if not k.startswith("_")}
    mono = TOKENS["font"]["mono"].replace('"', "'")
    rules = {
        "text": f"font-family:{mono};font-size:12px;fill:var(--ts-ink-muted,{c['ink-muted']})",
        "text.ts-strong": f"fill:var(--ts-ink,{c['ink']});font-weight:700",
        "text.ts-on-acid": "fill:#0A0A0A;font-weight:700",
        ".ts-axis": f"stroke:var(--ts-line,{c['line']});stroke-width:1;fill:none",
        ".ts-grid": f"stroke:var(--ts-line,{c['line']});stroke-width:1;stroke-dasharray:2 4;fill:none",
        ".ts-ink-line": f"stroke:var(--ts-ink,{c['ink']});stroke-width:2;fill:none",
        ".ts-muted-line": f"stroke:var(--ts-ink-muted,{c['ink-muted']});stroke-width:1.5;stroke-dasharray:5 4;fill:none",
        ".ts-ink-fill": f"fill:var(--ts-ink,{c['ink']})",
        ".ts-muted-fill": f"fill:var(--ts-ink-muted,{c['ink-muted']})",
        ".ts-unlit-fill": f"fill:var(--ts-unlit,{c['unlit']})",
        ".ts-ground-fill": f"fill:var(--ts-ground,{c['ground']})",
        ".ts-outline": f"fill:var(--ts-ground,{c['ground']});stroke:var(--ts-line,{c['line']});stroke-width:1",
        ".ts-ink-outline": f"fill:none;stroke:var(--ts-ink,{c['ink']});stroke-width:1.5",
        ".ts-acid": f"fill:var(--ts-acid,{c['acid']});stroke:var(--ts-keyline,{c['keyline']});stroke-width:1.5",
        ".ts-acid-line": f"stroke:var(--ts-acid,{c['acid']});stroke-width:2.5;fill:none",
        ".ts-ci": f"fill:var(--ts-ink,{c['ink']});opacity:.14",
        ".st-working": f"fill:var(--ts-ink,{c['ink']})",
        ".st-starved": f"fill:var(--ts-ground,{c['ground']});stroke:var(--ts-line,{c['line']});stroke-width:1",
        ".st-blocked-bg": f"fill:var(--ts-ground,{c['ground']})",
        ".st-pattern-hatch": f"stroke:var(--ts-ink-muted,{c['ink-muted']});stroke-width:1.6",
        ".st-down": f"fill:var(--ts-signal,{c['signal']})",
        ".st-pattern-dots": f"fill:var(--ts-ink-muted,{c['ink-muted']})",
    }
    return "".join(f".ts-svg {sel}{{{body}}}" for sel, body in rules.items())


def num(x: float) -> str:
    """A coordinate with at most one decimal, so files stay stable and small."""
    r = round(x, 1)
    return str(int(r)) if r == int(r) else f"{r:.1f}"


@dataclass
class Svg:
    """One figure. `uid` keeps pattern ids unique when figures share a page."""

    uid: str
    height: float
    title: str
    desc: str
    parts: list[str] = field(default_factory=list)
    patterns: bool = False

    def add(self, s: str) -> None:
        self.parts.append(s)

    def rect(self, x: float, y: float, w: float, h: float, cls: str = "", fill: str = "") -> None:
        a = f' class="{cls}"' if cls else ""
        f = f' fill="{fill}"' if fill else ""
        self.add(f'<rect x="{num(x)}" y="{num(y)}" width="{num(w)}" height="{num(h)}"{a}{f}/>')

    def line(self, x1: float, y1: float, x2: float, y2: float, cls: str) -> None:
        self.add(f'<line x1="{num(x1)}" y1="{num(y1)}" x2="{num(x2)}" y2="{num(y2)}" class="{cls}"/>')

    def path(self, d: str, cls: str) -> None:
        self.add(f'<path d="{d}" class="{cls}"/>')

    def text(self, x: float, y: float, s: str, anchor: str = "start", cls: str = "", size: float = 0) -> None:
        a = f' class="{cls}"' if cls else ""
        z = f' font-size="{num(size)}"' if size else ""
        self.add(f'<text x="{num(x)}" y="{num(y)}" text-anchor="{anchor}"{a}{z}>{escape(s)}</text>')

    def state_rect(self, x: float, y: float, w: float, h: float, state: str) -> None:
        """A cell in a machine state: solid, outline, hatch, signal or dots."""
        if state == "working":
            self.rect(x, y, w, h, "st-working")
        elif state == "starved":
            self.rect(x + 0.5, y + 0.5, w - 1, h - 1, "st-starved")
        elif state == "blocked":
            self.patterns = True
            self.rect(x, y, w, h, fill=f"url(#{self.uid}-hatch)")
        elif state == "down":
            self.rect(x, y, w, h, "st-down")
        elif state == "setup":
            self.patterns = True
            self.rect(x, y, w, h, fill=f"url(#{self.uid}-dots)")
        else:
            raise ValueError(state)

    def render(self) -> str:
        defs = ""
        if self.patterns:
            defs = (
                f'<defs><pattern id="{self.uid}-hatch" patternUnits="userSpaceOnUse" width="5" height="5"'
                ' patternTransform="rotate(45)"><rect width="5" height="5" class="st-blocked-bg"/>'
                '<line x1="0" y1="0" x2="0" y2="5" class="st-pattern-hatch"/></pattern>'
                f'<pattern id="{self.uid}-dots" patternUnits="userSpaceOnUse" width="4" height="4">'
                '<rect width="4" height="4" class="st-blocked-bg"/>'
                '<rect x="1" y="1" width="2" height="2" class="st-pattern-dots"/></pattern></defs>'
            )
        head = (
            f'<svg xmlns="http://www.w3.org/2000/svg" class="ts-svg" viewBox="0 0 {W} {num(self.height)}"'
            f' role="img" aria-labelledby="{self.uid}-t {self.uid}-d">'
            f'<title id="{self.uid}-t">{escape(self.title)}</title>'
            f'<desc id="{self.uid}-d">{escape(self.desc)}</desc>'
            f"<style>{_fallback_style()}</style>{defs}"
        )
        # One element per line and no blank lines: the file is inlined into
        # Markdown, where a blank line would end the HTML block.
        return head + "\n" + "\n".join(self.parts) + "\n</svg>\n"


# ------------------------------------------------------------ chart parts


@dataclass
class Scale:
    d0: float
    d1: float
    r0: float
    r1: float

    def __call__(self, v: float) -> float:
        return self.r0 + (v - self.d0) / (self.d1 - self.d0) * (self.r1 - self.r0)


def nice_ticks(lo: float, hi: float, count: int = 5) -> list[float]:
    span = hi - lo
    raw = span / count
    mag = 10 ** math.floor(math.log10(raw))
    step = min((m * mag for m in (1, 2, 2.5, 5, 10) if m * mag >= raw), default=10 * mag)
    first = math.ceil(lo / step - 1e-9) * step
    out = []
    v = first
    while v <= hi + 1e-9:
        out.append(round(v, 10))
        v += step
    return out


def fmt_tick(v: float) -> str:
    return f"{v:g}" if abs(v) < 1e5 else f"{v:.0f}"


@dataclass
class Series:
    """y values over x; `ci` gives half-widths drawn as bands."""

    label: str
    xs: Sequence[float]
    ys: Sequence[float]
    ci: Sequence[float] | None = None
    style: str = "ink"  # ink: data, line + square markers; ref: formula, dashed
    markers: bool = True


def xy_chart(
    uid: str,
    title: str,
    desc: str,
    series: Sequence[Series],
    xlabel: str,
    ylabel: str,
    xr: tuple[float, float],
    yr: tuple[float, float],
    highlight: tuple[float, float, str] | None = None,
    label_below: bool = False,
    hlines: Iterable[tuple[float, str]] = (),
    vlines: Iterable[tuple[float, str]] = (),
    height: float = 300,
    xticks: Sequence[float] | None = None,
    yticks: Sequence[float] | None = None,
) -> str:
    """A line chart with hairline axes; `highlight` puts the one acid mark."""
    svg = Svg(uid, height, title, desc)
    left, right, top, bottom = 64, 24, 20, 48
    sx = Scale(xr[0], xr[1], left, W - right)
    sy = Scale(yr[0], yr[1], height - bottom, top)
    for t in yticks if yticks is not None else nice_ticks(*yr):
        y = sy(t)
        svg.line(left, y, W - right, y, "ts-grid")
        svg.text(left - 8, y + 4, fmt_tick(t), "end")
    for t in xticks if xticks is not None else nice_ticks(*xr):
        x = sx(t)
        svg.line(x, height - bottom, x, height - bottom + 4, "ts-axis")
        svg.text(x, height - bottom + 18, fmt_tick(t), "middle")
    svg.line(left, height - bottom, W - right, height - bottom, "ts-axis")
    svg.line(left, top, left, height - bottom, "ts-axis")
    svg.text((left + W - right) / 2, height - 8, xlabel.upper(), "middle")
    svg.add(
        f'<text x="14" y="{num((top + height - bottom) / 2)}" text-anchor="middle"'
        f' transform="rotate(-90 14 {num((top + height - bottom) / 2)})">{escape(ylabel.upper())}</text>'
    )
    for v, label in hlines:
        svg.line(left, sy(v), W - right, sy(v), "ts-muted-line")
        svg.text(W - right - 4, sy(v) - 6, label, "end")
    for v, label in vlines:
        svg.line(sx(v), top, sx(v), height - bottom, "ts-muted-line")
        svg.text(sx(v) + 6, top + 12, label)
    for s in series:
        pts = [(sx(x), sy(y)) for x, y in zip(s.xs, s.ys, strict=True) if math.isfinite(y) and yr[0] <= y <= yr[1]]
        if s.ci is not None:
            up = [(sx(x), sy(min(y + h, yr[1]))) for x, y, h in zip(s.xs, s.ys, s.ci, strict=True)]
            lo = [(sx(x), sy(max(y - h, yr[0]))) for x, y, h in zip(s.xs, s.ys, s.ci, strict=True)]
            d = "M" + " L".join(f"{num(a)} {num(b)}" for a, b in up + lo[::-1]) + " Z"
            svg.path(d, "ts-ci")
        d = "M" + " L".join(f"{num(a)} {num(b)}" for a, b in pts)
        svg.path(d, "ts-ink-line" if s.style == "ink" else "ts-muted-line")
        if s.markers and s.style == "ink":
            for a, b in pts:
                svg.rect(a - 3, b - 3, 6, 6, "ts-ink-fill")
    if highlight is not None:
        hx, hy, label = highlight
        x, y = sx(hx), sy(hy)
        svg.rect(x - 6, y - 6, 12, 12, "ts-acid")
        anchor = "end" if x > W * 0.7 else "start"
        svg.text(x + (-12 if anchor == "end" else 12), y + (22 if label_below else -10), label, anchor, "ts-strong")
    legend = [s for s in series if s.label]
    lx = left + 12
    for s in legend:
        if s.style == "ink":
            svg.rect(lx, top + 2, 10, 10, "ts-ink-fill")
        else:
            svg.line(lx, top + 7, lx + 14, top + 7, "ts-muted-line")
        svg.text(lx + 18, top + 11, s.label.upper())
        lx += 30 + 8 * len(s.label)
    return svg.render()


def timeline(
    uid: str,
    title: str,
    desc: str,
    rows: Sequence[tuple[str, list[tuple[float, str]]]],
    t0: float,
    t1: float,
    cells: int = 60,
    levels: Sequence[tuple[str, list[tuple[float, int]], int]] = (),
    mark: tuple[float, str] | None = None,
) -> str:
    """Machine states as rows of square steps, one cell per time slice.

    Each cell shows the state the station spent most of its slice in.
    `levels` adds buffer fill charts aligned below; `mark` puts the one
    acid step on a moment worth looking at.
    """
    left, right, top = 92, 16, 52
    pitch = (W - left - right) / cells
    cell = pitch - 2
    row_h = cell + 12
    lvl_h = 54
    height = top + len(rows) * row_h + len(levels) * (lvl_h + 16) + 52
    svg = Svg(uid, height, title, desc)
    slice_ = (t1 - t0) / cells
    y = top
    # Legend.
    lx = left
    for st in ("working", "starved", "blocked", "down", "setup"):
        if any(s == st for _, tr in rows for _, s in tr):
            svg.state_rect(lx, 8, 10, 10, st)
            svg.text(lx + 16, 17, st.upper())
            lx += 24 + 8 * len(st) + 12
    for name, tr in rows:
        svg.text(left - 10, y + cell * 0.85, name.upper(), "end", "ts-strong")
        for i in range(cells):
            a, b = t0 + i * slice_, t0 + (i + 1) * slice_
            svg.state_rect(left + i * pitch, y, cell, cell, dominant(tr, a, b))
        y += row_h
    for name, tr, cap in levels:
        y += 16
        base = y + lvl_h
        svg.text(left - 10, y + 10, name.upper(), "end", "ts-strong")
        svg.text(left - 10, y + 24, f"max {cap}", "end")
        svg.line(left, base, W - right, base, "ts-axis")
        svg.line(left, y, W - right, y, "ts-grid")
        for i in range(cells):
            a, b = t0 + i * slice_, t0 + (i + 1) * slice_
            v = _mean_level(tr, a, b)
            h = 0 if cap == 0 else v / cap * lvl_h
            if h > 0.5:
                svg.rect(left + i * pitch, base - h, cell, h, "ts-muted-fill")
        y += lvl_h
    if mark is not None:
        mt, label = mark
        i = min(cells - 1, int((mt - t0) / slice_))
        x = left + i * pitch
        svg.rect(x, top - 10, cell, 6, "ts-acid")
        svg.line(x + cell / 2, top - 4, x + cell / 2, y, "ts-acid-line")
        if x < W * 0.6:
            svg.text(x + cell + 6, top - 4, label, "start", "ts-strong")
        else:
            svg.text(x - 6, top - 4, label, "end", "ts-strong")
    # Time axis.
    ty = y + 4
    for frac in (0, 0.25, 0.5, 0.75, 1):
        x = left + frac * (W - left - right)
        svg.text(x, ty + 14, f"{(t0 + frac * (t1 - t0)) / 60:g} min", "middle" if 0 < frac < 1 else ("start" if frac == 0 else "end"))
    return svg.render()


def dominant(trace: list[tuple[float, str]], a: float, b: float) -> str:
    """The state with the most time in [a, b] of a step trace."""
    time: dict[str, float] = {}
    for i, (t, s) in enumerate(trace):
        end = trace[i + 1][0] if i + 1 < len(trace) else math.inf
        lo, hi = max(t, a), min(end, b)
        if hi > lo:
            time[s] = time.get(s, 0.0) + hi - lo
    order = ("down", "setup", "blocked", "starved", "working")
    return max(order, key=lambda s: (time.get(s, 0.0), -order.index(s)))


def _mean_level(trace: list[tuple[float, int]], a: float, b: float) -> float:
    area = 0.0
    for i, (t, v) in enumerate(trace):
        end = trace[i + 1][0] if i + 1 < len(trace) else math.inf
        lo, hi = max(t, a), min(end, b)
        if hi > lo:
            area += v * (hi - lo)
    return area / (b - a)


def histograms(
    uid: str,
    title: str,
    desc: str,
    panels: Sequence[tuple[str, Sequence[float]]],
    lo: float,
    hi: float,
    bins: int,
    xlabel: str,
    mark: tuple[int, float, str] | None = None,
    height: float = 220,
) -> str:
    """Side-by-side histograms on one x scale, bars as square-cornered steps."""
    svg = Svg(uid, height, title, desc)
    gap = 32
    pw = (W - 24 - gap * (len(panels) - 1)) / len(panels)
    top, bottom = 34, 44
    counts_all = []
    for _, data in panels:
        c = [0] * bins
        for v in data:
            if lo <= v < hi:
                c[int((v - lo) / (hi - lo) * bins)] += 1
        counts_all.append(c)
    peak = max(max(c) for c in counts_all)
    for p, ((name, _), c) in enumerate(zip(panels, counts_all, strict=True)):
        x0 = 12 + p * (pw + gap)
        base = height - bottom
        svg.text(x0, 18, name.upper(), "start", "ts-strong")
        bw = pw / bins
        for i, k in enumerate(c):
            h = k / peak * (base - top)
            if h > 0:
                svg.rect(x0 + i * bw + 0.5, base - h, bw - 1, h, "ts-ink-fill")
        svg.line(x0, base, x0 + pw, base, "ts-axis")
        for t in nice_ticks(lo, hi, 4):
            x = x0 + (t - lo) / (hi - lo) * pw
            svg.line(x, base, x, base + 4, "ts-axis")
            svg.text(x, base + 18, fmt_tick(t), "middle")
        svg.text(x0 + pw / 2, height - 6, xlabel.upper(), "middle")
        if mark is not None and mark[0] == p:
            mx = x0 + (mark[1] - lo) / (hi - lo) * pw
            svg.line(mx, top - 6, mx, base, "ts-acid-line")
            svg.text(mx + 6, top + 4, mark[2], "start", "ts-strong")
    return svg.render()


@dataclass
class Node:
    """One element of a line diagram."""

    kind: str  # source, station, buffer, sink
    name: str = ""
    sub: str = ""  # a short parameter line under the element
    highlight: bool = False  # the one acid element: usually the bottleneck
    capacity: int | None = None  # buffers: shown as slots
    fails: bool = False  # stations: a signal mark for failures


def line_diagram(
    uid: str,
    title: str,
    desc: str,
    nodes: Sequence[Node],
    loops: Sequence[tuple[int, int, str]] = (),
    exits: Sequence[tuple[int, str]] = (),
) -> str:
    """The running example's line, drawn the same way on every page.

    Stations are square-cornered blocks, buffers rows of slots, flow runs
    left to right. `loops` draws rework arcs (from, to, label) above the
    line, `exits` scrap arrows (at, label) below it.
    """
    n_st = sum(1 for n in nodes if n.kind == "station")
    st_w = 92 if n_st <= 2 else (76 if n_st <= 3 else 60)
    slot, slot_gap = 9, 3

    def width(n: Node) -> float:
        if n.kind == "station":
            return st_w
        if n.kind == "buffer":
            shown = min(n.capacity or 0, 5 if n_st <= 3 else 3)
            return max(30, shown * (slot + slot_gap) + 6)
        return 46

    gaps = len(nodes) - 1
    total = sum(width(n) for n in nodes)
    arrow = max(14, min(36, (W - 24 - total) / max(gaps, 1)))
    x = (W - total - arrow * gaps) / 2
    height = 150 + (24 if loops else 0) + (26 if exits else 0)
    mid = 74 + (24 if loops else 0)
    svg = Svg(uid, height, title, desc)
    xs: list[tuple[float, float]] = []
    for i, n in enumerate(nodes):
        w = width(n)
        xs.append((x, w))
        if n.kind == "station":
            h = 52
            if n.highlight:
                svg.rect(x, mid - h / 2, w, h, "ts-acid")
                svg.text(x + w / 2, mid + 4, n.name.upper(), "middle", "ts-on-acid", 11 if n_st > 3 else 12)
            else:
                svg.rect(x, mid - h / 2, w, h, "ts-ink-outline")
                svg.text(x + w / 2, mid + 4, n.name.upper(), "middle", "ts-strong", 11 if n_st > 3 else 12)
            if n.fails:
                svg.rect(x + w - 10, mid - h / 2 + 4, 6, 6, "st-down")
            if n.sub:
                for j, part in enumerate(n.sub.split("\n")):
                    svg.text(x + w / 2, mid + h / 2 + 18 + 14 * j, part, "middle", "", 11)
        elif n.kind == "buffer":
            cap = n.capacity or 0
            shown = min(cap, 5 if n_st <= 3 else 3)
            bx = x + (w - shown * (slot + slot_gap) + slot_gap) / 2
            for j in range(shown):
                svg.rect(bx + j * (slot + slot_gap), mid - slot / 2, slot, slot, "ts-unlit-fill")
            if cap == 0:
                svg.text(x + w / 2, mid + 4, "–", "middle")
            label = n.name or f"buf {cap}"
            svg.text(x + w / 2, mid - 14, label.upper(), "middle", "", 10)
            if n.sub:
                svg.text(x + w / 2, mid + 22, n.sub, "middle", "", 10)
        else:
            svg.text(x + w / 2, mid + 4, (n.name or n.kind).upper(), "middle", "", 11)
        if i < gaps:
            ax = x + w + 3
            bx2 = x + w + arrow - 3
            svg.line(ax, mid, bx2, mid, "ts-axis")
            svg.path(f"M{num(bx2 - 5)} {num(mid - 4)} L{num(bx2)} {num(mid)} L{num(bx2 - 5)} {num(mid + 4)}", "ts-axis")
        x += w + arrow
    for a, b, label in loops:
        (xa, wa), (xb, wb) = xs[a], xs[b]
        x1, x2 = xa + wa / 2, xb + wb / 2
        top = mid - 26 - 24
        svg.path(f"M{num(x1)} {num(mid - 26)} L{num(x1)} {num(top)} L{num(x2)} {num(top)} L{num(x2)} {num(mid - 31)}", "ts-axis")
        svg.path(f"M{num(x2 - 4)} {num(mid - 36)} L{num(x2)} {num(mid - 30)} L{num(x2 + 4)} {num(mid - 36)}", "ts-axis")
        svg.text((x1 + x2) / 2, top - 6, label.upper(), "middle", "", 10)
    for a, label in exits:
        xa, wa = xs[a]
        cx = xa + wa - 14
        y0, y1 = mid + 26, height - 22
        svg.line(cx, y0, cx, y1, "ts-axis")
        svg.path(f"M{num(cx - 4)} {num(y1 - 5)} L{num(cx)} {num(y1)} L{num(cx + 4)} {num(y1 - 5)}", "ts-axis")
        svg.text(cx + 8, y1 - 2, label.upper(), "start", "", 10)
    return svg.render()


def time_average(trace: Sequence[tuple[float, float]], t0: float, t1: float, slices: int) -> list[float]:
    """The time average of a step trace in each of `slices` equal slices."""
    out = []
    width = (t1 - t0) / slices
    for i in range(slices):
        a, b = t0 + i * width, t0 + (i + 1) * width
        area = 0.0
        for j, (t, v) in enumerate(trace):
            end = trace[j + 1][0] if j + 1 < len(trace) else math.inf
            lo, hi = max(t, a), min(end, b)
            if hi > lo:
                area += v * (hi - lo)
        out.append(area / (b - a))
    return out


def matrix_chart(
    uid: str,
    title: str,
    desc: str,
    values: Sequence[float],
    rows: int,
    xlabel: str,
    ylabel: str,
    xticks: Sequence[tuple[int, str]],
    mark: tuple[int, str] | None = None,
    unit: float = 1.0,
) -> str:
    """Columns of square steps, lit up to each value: the brand's dot matrix.

    One column per slice; a cell is lit when the value reaches it (each
    cell stands for `unit`). `mark` lights the one acid cell, at the top
    of the given column.
    """
    left, right, top, bottom = 64, 16, 16, 48
    cols = len(values)
    pitch_x = (W - left - right) / cols
    height = 300
    pitch_y = (height - top - bottom) / rows
    pitch = min(pitch_x, pitch_y)
    cell = pitch * 0.8
    height = top + rows * pitch + bottom
    svg = Svg(uid, height, title, desc)
    base = top + rows * pitch
    for c, v in enumerate(values):
        lit = round(v / unit)
        x = left + c * pitch_x + (pitch_x - cell) / 2
        for r in range(rows):
            y = base - (r + 1) * pitch + (pitch - cell) / 2
            if mark is not None and c == mark[0] and r == lit - 1:
                svg.rect(x, y, cell, cell, "ts-acid")
            else:
                svg.rect(x, y, cell, cell, "ts-ink-fill" if r < lit else "ts-unlit-fill")
    for r in range(0, rows + 1, 5):
        svg.text(left - 8, base - r * pitch + 4, fmt_tick(r * unit), "end")
    for c, label in xticks:
        svg.text(left + (c + 0.5) * pitch_x, base + 18, label, "middle")
    svg.text((left + W - right) / 2, height - 8, xlabel.upper(), "middle")
    my = (top + base) / 2
    svg.add(f'<text x="14" y="{num(my)}" text-anchor="middle" transform="rotate(-90 14 {num(my)})">{escape(ylabel.upper())}</text>')
    if mark is not None:
        c, label = mark
        lit = round(values[c] / unit)
        x = left + (c + 0.5) * pitch_x
        y = base - lit * pitch - 8
        anchor = "end" if x > W * 0.7 else "start"
        svg.text(x + (-10 if anchor == "end" else 10), y, label, anchor, "ts-strong")
    return svg.render()
