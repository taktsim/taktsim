// A small discrete-event simulator of serial production lines, for the
// interactive figures of the concept pages. A teaching tool, not the taktsim
// engine: it is a line-by-line port of docs/concepts/scripts/linesim.py,
// which documents the model. Keep the two in step; sim.test.js checks that
// they agree.
(function (root) {
  "use strict";

  // ------------------------------------------------------------- random

  function splitmix32(x) {
    x = (x + 0x9e3779b9) >>> 0;
    let z = x;
    z = Math.imul(z ^ (z >>> 16), 0x85ebca6b) >>> 0;
    z = Math.imul(z ^ (z >>> 13), 0xc2b2ae35) >>> 0;
    return [x, (z ^ (z >>> 16)) >>> 0];
  }

  function rotl(x, k) {
    return ((x << k) | (x >>> (32 - k))) >>> 0;
  }

  // xoshiro128** seeded by SplitMix32 from (seed, stream id).
  class Stream {
    constructor(seed, stream) {
      let x = (Math.imul(seed, 0x9e3779b1) + Math.imul(stream, 0x85ebca77) + 1) >>> 0;
      this.s = [];
      for (let i = 0; i < 4; i++) {
        const r = splitmix32(x);
        x = r[0];
        this.s.push(r[1]);
      }
      if (this.s.every((v) => v === 0)) this.s[0] = 1;
    }

    next32() {
      const s = this.s;
      const result = Math.imul(rotl(Math.imul(s[1], 5) >>> 0, 7), 9) >>> 0;
      const t = (s[1] << 9) >>> 0;
      s[2] = (s[2] ^ s[0]) >>> 0;
      s[3] = (s[3] ^ s[1]) >>> 0;
      s[1] = (s[1] ^ s[2]) >>> 0;
      s[0] = (s[0] ^ s[3]) >>> 0;
      s[2] = (s[2] ^ t) >>> 0;
      s[3] = rotl(s[3], 11);
      return result;
    }

    // A float in [0, 1) with 53 random bits.
    uniform() {
      const a = this.next32() >>> 5;
      const b = this.next32() >>> 6;
      return (a * 67108864 + b) / 9007199254740992;
    }
  }

  // A non-negative random duration: const, exp or lognormal (by mean).
  const Dist = {
    const: (mean) => ({ kind: "const", mean, sigma: 0 }),
    exp: (mean) => ({ kind: "exp", mean, sigma: 0 }),
    lognormal: (mean, sigma) => (sigma > 0 ? { kind: "lognormal", mean, sigma } : Dist.const(mean)),
    lognormalCv: (mean, cv) => Dist.lognormal(mean, Math.sqrt(Math.log(1 + cv * cv))),
  };

  function sample(d, rng) {
    if (d.kind === "const") return d.mean;
    if (d.kind === "exp") return -d.mean * Math.log(1.0 - rng.uniform());
    const u1 = 1.0 - rng.uniform();
    const u2 = rng.uniform();
    const z = Math.sqrt(-2.0 * Math.log(u1)) * Math.cos(2.0 * Math.PI * u2);
    const mu = Math.log(d.mean) - (d.sigma * d.sigma) / 2.0;
    return Math.exp(mu + d.sigma * z);
  }

  // -------------------------------------------------------------- model

  // station: {name, time, mtbf, mttr, setup, batch, scrap, rework, reworkTo}
  // line: {stations, buffers, arrivals, conwip}
  function station(name, time, opts) {
    return Object.assign(
      { name, time, mtbf: null, mttr: null, setup: 0, batch: 1, scrap: 0, rework: 0, reworkTo: null },
      opts || {},
    );
  }

  const PROCESS = 1, FAIL = 2, REPAIR = 3, INSPECT = 4, ARRIVALS = 9;
  const STATES = ["working", "starved", "blocked", "down", "setup"];

  // A binary heap of events ordered by (time, sequence).
  class Heap {
    constructor() {
      this.a = [];
    }
    push(e) {
      const a = this.a;
      a.push(e);
      let i = a.length - 1;
      while (i > 0) {
        const p = (i - 1) >> 1;
        if (less(a[i], a[p])) {
          [a[i], a[p]] = [a[p], a[i]];
          i = p;
        } else break;
      }
    }
    pop() {
      const a = this.a;
      const top = a[0];
      const last = a.pop();
      if (a.length) {
        a[0] = last;
        let i = 0;
        for (;;) {
          const l = 2 * i + 1, r = l + 1;
          let m = i;
          if (l < a.length && less(a[l], a[m])) m = l;
          if (r < a.length && less(a[r], a[m])) m = r;
          if (m === i) break;
          [a[i], a[m]] = [a[m], a[i]];
          i = m;
        }
      }
      return top;
    }
    get size() {
      return this.a.length;
    }
  }

  function less(x, y) {
    return x.t < y.t || (x.t === y.t && x.seq < y.seq);
  }

  function simulate(line, horizon, seed, warmup, trace) {
    warmup = warmup || 0;
    const st = line.stations;
    const n = st.length;
    const res = {
      horizon, warmup, completed: 0, scrapped: 0, leadTimeSum: 0, wipArea: 0,
      queueWaitSum: 0, queueWaitN: 0,
      stateTime: st.map(() => Object.fromEntries(STATES.map((s) => [s, 0]))),
      states: [], levels: [], wipTrace: [],
    };
    const proc = st.map((_, k) => new Stream(seed, 10 * k + PROCESS));
    const fail = st.map((_, k) => new Stream(seed, 10 * k + FAIL));
    const repair = st.map((_, k) => new Stream(seed, 10 * k + REPAIR));
    const insp = st.map((_, k) => new Stream(seed, 10 * k + INSPECT));
    const arr = new Stream(seed, ARRIVALS);
    const s = st.map(() => ({
      held: [], phase: "", down: false, token: 0, end: 0, remaining: 0,
      rework: [], state: "starved", lastChange: 0,
    }));
    const bufs = [];
    for (let k = 0; k < n - 1; k++) bufs.push([]);
    const queue = [];
    const events = new Heap();
    let seq = 0, now = 0, wip = 0, wipSince = 0;
    if (trace) {
      res.states = st.map(() => [[0, "starved"]]);
      res.levels = bufs.map(() => [[0, 0]]);
      res.wipTrace = [[0, 0]];
    }

    const push = (t, kind, k, token) => events.push({ t, seq: seq++, kind, k, token: token || 0 });
    const inWindow = (a, b) => Math.max(0, Math.min(b, horizon) - Math.max(a, warmup));

    function setState(k, next) {
      const x = s[k];
      if (next === x.state) return;
      res.stateTime[k][x.state] += inWindow(x.lastChange, now);
      x.state = next;
      x.lastChange = now;
      if (trace) res.states[k].push([now, next]);
    }

    function stateOf(k) {
      const x = s[k];
      if (x.down) return "down";
      if (x.phase === "setup") return "setup";
      if (x.phase === "work") return "working";
      if (x.phase === "done") return "blocked";
      return "starved";
    }

    function changeWip(d) {
      res.wipArea += wip * inWindow(wipSince, now);
      wip += d;
      wipSince = now;
      if (trace) res.wipTrace.push([now, wip]);
    }

    function level(b) {
      if (trace) res.levels[b].push([now, bufs[b].length]);
    }

    function workTime(k, count) {
      let total = 0;
      for (let i = 0; i < count; i++) total += sample(st[k].time, proc[k]);
      return total;
    }

    function start(k, parts) {
      const x = s[k];
      x.held = parts;
      if (k === 0 && line.arrivals) {
        for (const p of parts) {
          if (now >= warmup) {
            res.queueWaitSum += now - p[1];
            res.queueWaitN += 1;
          }
        }
      }
      if (st[k].setup > 0) {
        x.phase = "setup";
        x.end = now + st[k].setup;
      } else {
        x.phase = "work";
        x.end = now + workTime(k, parts.length);
      }
      x.token += 1;
      push(x.end, "end", k, x.token);
      setState(k, stateOf(k));
    }

    const empty = (k) => s[k].held.length === 0 && !s[k].down;

    function tryPull(k) {
      const x = s[k];
      if (x.held.length || x.down) return false;
      if (x.rework.length) {
        start(k, [x.rework.shift()]);
        return true;
      }
      if (k > 0) {
        if (bufs[k - 1].length) {
          const part = bufs[k - 1].shift();
          level(k - 1);
          start(k, [[part[0], now]]);
          return true;
        }
        return false;
      }
      if (line.arrivals) {
        const b = st[0].batch;
        if (queue.length >= b) {
          start(0, queue.splice(0, b));
          return true;
        }
        return false;
      }
      if (line.conwip != null && wip >= line.conwip) return false;
      changeWip(+1);
      start(0, [[now, now]]);
      return true;
    }

    function tryPush(k) {
      const x = s[k];
      if (x.phase !== "done" || x.down) return false;
      if (k === n - 1) {
        for (const p of x.held) {
          if (now >= warmup && now <= horizon) {
            res.completed += 1;
            res.leadTimeSum += now - p[0];
          }
          changeWip(-1);
        }
        x.held = [];
        x.phase = "";
        setState(k, stateOf(k));
        return true;
      }
      const cap = line.buffers[k];
      if (!bufs[k].length && empty(k + 1) && !s[k + 1].rework.length) {
        const part = x.held[0];
        x.held = [];
        x.phase = "";
        setState(k, stateOf(k));
        start(k + 1, [[part[0], now]]);
        return true;
      }
      if (cap == null || bufs[k].length < cap) {
        bufs[k].push(x.held[0]);
        level(k);
        x.held = [];
        x.phase = "";
        setState(k, stateOf(k));
        return true;
      }
      return false;
    }

    function settle() {
      let moved = true;
      while (moved) {
        moved = false;
        for (let k = n - 1; k >= 0; k--) if (tryPush(k)) moved = true;
        for (let k = 0; k < n; k++) if (tryPull(k)) moved = true;
      }
    }

    function finishWork(k) {
      const x = s[k];
      const sp = st[k];
      if (sp.scrap > 0 || sp.rework > 0) {
        const u = insp[k].uniform();
        if (u < sp.scrap) {
          if (now >= warmup && now <= horizon) res.scrapped += 1;
          x.held = [];
          x.phase = "";
          changeWip(-1);
          setState(k, stateOf(k));
          return;
        }
        if (u < sp.scrap + sp.rework) {
          const target = sp.reworkTo == null ? k : sp.reworkTo;
          const part = x.held[0];
          if (target === k) {
            x.phase = "work";
            x.end = now + sample(sp.time, proc[k]);
            x.token += 1;
            push(x.end, "end", k, x.token);
            return;
          }
          s[target].rework.push([part[0], now]);
          x.held = [];
          x.phase = "";
          setState(k, stateOf(k));
          return;
        }
      }
      x.phase = "done";
      setState(k, stateOf(k));
    }

    st.forEach((sp, k) => {
      if (sp.mtbf != null) push(-sp.mtbf * Math.log(1.0 - fail[k].uniform()), "fail", k);
    });
    if (line.arrivals) push(sample(line.arrivals, arr), "arrive", 0);
    settle();

    while (events.size) {
      const e = events.pop();
      if (e.t > horizon) break;
      now = e.t;
      const k = e.k;
      const x = s[k];
      if (e.kind === "end") {
        if (e.token !== x.token || x.down) continue;
        if (x.phase === "setup") {
          x.phase = "work";
          x.end = now + workTime(k, x.held.length);
          x.token += 1;
          push(x.end, "end", k, x.token);
          setState(k, stateOf(k));
        } else {
          finishWork(k);
        }
      } else if (e.kind === "fail") {
        x.down = true;
        if (x.phase === "setup" || x.phase === "work") {
          x.remaining = x.end - now;
          x.token += 1;
        }
        setState(k, "down");
        push(now - (st[k].mttr || 0) * Math.log(1.0 - repair[k].uniform()), "repair", k);
      } else if (e.kind === "repair") {
        x.down = false;
        if (x.phase === "setup" || x.phase === "work") {
          x.end = now + x.remaining;
          x.token += 1;
          push(x.end, "end", k, x.token);
        }
        setState(k, stateOf(k));
        push(now - (st[k].mtbf || 0) * Math.log(1.0 - fail[k].uniform()), "fail", k);
      } else if (e.kind === "arrive") {
        changeWip(+1);
        queue.push([now, now]);
        push(now + sample(line.arrivals, arr), "arrive", 0);
      }
      settle();
    }

    now = horizon;
    for (let k = 0; k < n; k++) res.stateTime[k][s[k].state] += inWindow(s[k].lastChange, now);
    res.wipArea += wip * inWindow(wipSince, now);

    const span = horizon - warmup;
    res.throughput = (res.completed / span) * 3600;
    res.wip = res.wipArea / span;
    res.leadTime = res.completed ? res.leadTimeSum / res.completed : NaN;
    res.queueWait = res.queueWaitN ? res.queueWaitSum / res.queueWaitN : NaN;
    res.share = (k, state) => res.stateTime[k][state] / span;
    return res;
  }

  // ------------------------------------------------------- replications

  const T95 = { 1: 12.706, 2: 4.303, 3: 3.182, 4: 2.776, 5: 2.571, 6: 2.447, 7: 2.365, 8: 2.306, 9: 2.262,
    10: 2.228, 14: 2.145, 19: 2.093, 24: 2.064, 29: 2.045, 39: 2.023, 49: 2.01, 99: 1.984 };

  function t95(df) {
    if (T95[df] !== undefined) return T95[df];
    if (df > 99) return 1.96 + 2.4 / df;
    const keys = Object.keys(T95).map(Number);
    const lower = Math.max(...keys.filter((d) => d < df));
    const upper = Math.min(...keys.filter((d) => d > df));
    const w = (df - lower) / (upper - lower);
    return T95[lower] * (1 - w) + T95[upper] * w;
  }

  // Mean of replications with a 95 % confidence interval (Student t).
  function estimate(values) {
    const n = values.length;
    const mean = values.reduce((a, b) => a + b, 0) / n;
    if (n < 2) return { mean, half: NaN, n };
    const v = values.reduce((a, b) => a + (b - mean) * (b - mean), 0) / (n - 1);
    return { mean, half: t95(n - 1) * Math.sqrt(v / n), n };
  }

  function replicate(line, horizon, reps, warmup, seed0) {
    const out = [];
    for (let r = 0; r < reps; r++) out.push(simulate(line, horizon, (seed0 || 1) + r, warmup || 0, false));
    return out;
  }

  const api = { Stream, Dist, sample, station, simulate, estimate, replicate, t95, STATES };
  if (typeof module !== "undefined" && module.exports) module.exports = api;
  else root.TaktsimSim = api;
})(typeof self !== "undefined" ? self : this);
