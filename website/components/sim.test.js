// Tests of the in-browser teaching simulator (run: node --test website/components/).
// It must reproduce what the Python scripts in docs/concepts/scripts/ report
// on the pages: the same random streams, and for every interactive figure the
// same results within the scripts' 95 % confidence intervals.
"use strict";
const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const S = require("./sim.js");

const DATA = path.join(__dirname, "..", "..", "docs", "concepts", "data");
const load = (name) => JSON.parse(fs.readFileSync(path.join(DATA, name), "utf8"));

const lineA = (sigma, buffer, weldSigma) => ({
  stations: [
    S.station("cut", S.Dist.lognormal(42, sigma)),
    S.station("weld", S.Dist.lognormal(45, weldSigma == null ? sigma : weldSigma)),
  ],
  buffers: [buffer],
});

// |js − python| must lie within the Python estimate's confidence interval.
function within(js, py, half, what) {
  assert.ok(Math.abs(js - py) <= half, `${what}: simulator ${js.toFixed(3)}, scripts ${py.toFixed(3)} ± ${half.toFixed(3)}`);
}

test("random streams equal the Python ones", () => {
  // Values printed by docs/concepts/scripts/linesim.py: Stream(7, 11), Stream(1, 1).
  const s = new S.Stream(7, 11);
  assert.deepEqual([s.next32(), s.next32(), s.next32()], [2763008736, 2087911731, 1540876577]);
  assert.equal(new S.Stream(1, 1).uniform(), 0.766357292377538);
});

test("same seed, same run", () => {
  const a = S.simulate(lineA(0.5, 1), 28800, 3);
  const b = S.simulate(lineA(0.5, 1), 28800, 3);
  assert.equal(a.completed, b.completed);
  assert.equal(a.wip, b.wip);
});

test("conservation: parts in = parts out + parts inside", () => {
  const line = {
    stations: [
      S.station("cut", S.Dist.lognormal(42, 0.5), { mtbf: 3600, mttr: 300 }),
      S.station("weld", S.Dist.lognormal(45, 0.5), { scrap: 0.05, rework: 0.1 }),
    ],
    buffers: [3],
  };
  const r = S.simulate(line, 28800, 5, 0, true);
  const created = r.wipTrace.filter((e, i) => i > 0 && e[1] > r.wipTrace[i - 1][1]).length;
  const inside = r.wipTrace[r.wipTrace.length - 1][1];
  assert.equal(created, r.completed + r.scrapped + inside);
});

test("page 5: wait in queue matches the scripts", () => {
  const d = load("05-waiting.json");
  for (const c of d.results) {
    const proc = c.cv === 1 ? S.Dist.exp(d.process_mean) : S.Dist.lognormalCv(d.process_mean, c.cv);
    const line = { stations: [S.station("weld", proc)], buffers: [], arrivals: S.Dist.exp(d.process_mean / c.u) };
    const e = S.estimate(S.replicate(line, d.horizon, d.reps, d.warmup).map((r) => r.queueWait / 60));
    within(e.mean, c.wait_min, c.half_min, `cv ${c.cv}, u ${c.u}`);
  }
});

test("page 6: state shares match the scripts", () => {
  const d = load("06-shares.json");
  for (const c of d.results) {
    const rs = S.replicate(lineA(c.sigma, c.buffer), d.horizon, d.reps);
    const e = S.estimate(rs.map((r) => r.completed));
    within(e.mean, c.completed, Math.max(c.completed_half, 1e-9), `sigma ${c.sigma}, parts`);
    const starved = rs.reduce((a, r) => a + r.share(1, "starved"), 0) / rs.length;
    assert.ok(Math.abs(starved - c.weld_starved) < 0.005, `sigma ${c.sigma}, weld starved`);
  }
});

test("page 7: parts per shift against buffer size match the scripts", () => {
  const d = load("07-buffers.json");
  for (const c of d.results) {
    const e = S.estimate(S.replicate(lineA(d.sigma, c.buffer), d.horizon, d.reps).map((r) => r.completed));
    within(e.mean, c.completed, c.completed_half, `buffer ${c.buffer}`);
    const rd = S.estimate(S.replicate(lineA(0.08, c.buffer, 0), d.horizon, d.reps).map((r) => r.completed));
    within(rd.mean, c.readme_completed, c.readme_half, `README line, buffer ${c.buffer}`);
  }
});
