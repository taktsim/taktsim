// Tests of the in-browser teaching simulator (run: node --test website/components/sim.test.js).
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
    within(rd.mean, c.low_completed, c.low_half, `low variability, buffer ${c.buffer}`);
  }
});

test("page 8: failures at equal availability match the scripts", () => {
  const d = load("08-failures.json");
  for (const c of d.results) {
    const line = {
      stations: [
        S.station("cut", S.Dist.lognormal(42, d.sigma), { mtbf: c.mtbf, mttr: c.mttr }),
        S.station("weld", S.Dist.lognormal(45, d.sigma)),
      ],
      buffers: [c.buffer],
    };
    const e = S.estimate(S.replicate(line, d.horizon, d.reps).map((r) => r.completed / d.per));
    within(e.mean, c.completed, c.completed_half, `MTBF ${c.mtbf}, buffer ${c.buffer}`);
  }
});

test("page 10: CONWIP throughput matches the scripts", () => {
  const d = load("10-conwip.json");
  const names = [["cut", 42], ["deburr", 40], ["weld", 45], ["paint", 43], ["pack", 38]];
  for (const c of d.results) {
    const line = {
      stations: names.map(([n, m]) => S.station(n, S.Dist.lognormal(m, d.sigma))),
      buffers: [null, null, null, null],
      conwip: c.cap,
    };
    const e = S.estimate(S.replicate(line, d.horizon, d.reps, d.warmup).map((r) => r.completed * d.per_shift));
    within(e.mean, c.completed, c.completed_half, `cap ${c.cap}`);
  }
});

test("page 11: lead time against batch size matches the scripts", () => {
  const d = load("11-batching.json");
  for (const c of d.results) {
    const line = {
      stations: [S.station("weld", S.Dist.lognormal(45, d.sigma), { setup: d.setup, batch: c.batch })],
      buffers: [],
      arrivals: S.Dist.exp(d.arrival),
    };
    const e = S.estimate(S.replicate(line, d.horizon, d.reps, d.warmup).map((r) => r.leadTime / 60));
    within(e.mean, c.lead_min, c.lead_half, `batch ${c.batch}`);
  }
});

test("page 13: the replications match the scripts", () => {
  const d = load("13-replications.json");
  const line = {
    stations: [
      S.station("cut", S.Dist.lognormal(42, 0.5), { mtbf: 3600, mttr: 180 }),
      S.station("weld", S.Dist.lognormal(45, 0.5)),
    ],
    buffers: [20],
  };
  const js = S.replicate(line, d.horizon, d.values.length).map((r) => r.completed);
  const py = S.estimate(d.values);
  within(S.estimate(js).mean, py.mean, py.half, "100 shifts");
});
