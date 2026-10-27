"""A small discrete-event simulator of serial production lines.

This is a teaching tool for the concept pages, independent of taktsim: it
produces the numbers and figures on those pages so that every one of them
can be reproduced. website/components/sim.js is a line-by-line port of it
for the interactive figures; keep the two in step.

The model:

- Stations in a row. Each holds at most one part (or one batch) and has a
  process time distribution. Between stations k and k + 1 sits a buffer
  of a given capacity (None: unlimited).
- The source either keeps the first station always supplied (saturated),
  or parts arrive with a given interarrival distribution into an
  unlimited queue in front of the first station. A CONWIP cap limits how
  many parts are in the line at once (saturated source only).
- Blocking after service: a finished part stays on its station until the
  next buffer has room, and the station is blocked meanwhile.
- Failures count calendar time (spec §4, v0.1): time between failures and
  time to repair are exponential; a failure pauses the operation in
  progress, which resumes after the repair. A down station neither takes
  nor releases parts.
- Setups: a station with a setup time sets up before every batch.
  Batching (batch > 1) is supported on the first station with arrivals:
  it waits until `batch` parts are queued and releases them together.
- Inspection: after processing, a part is scrapped with probability
  `scrap`, or sent back to station `rework_to` (an unlimited rework queue
  served before the normal input) with probability `rework`.

Random numbers come from one stream per station and purpose (process,
failure, repair, inspection, arrivals), as in taktsim itself.
"""

from __future__ import annotations

import heapq
import math
from collections import deque
from dataclasses import dataclass, field

MASK32 = 0xFFFFFFFF

# ----------------------------------------------------------------- random


def _splitmix32(x: int) -> tuple[int, int]:
    """One step of a 32-bit SplitMix-style generator: (new state, output)."""
    x = (x + 0x9E3779B9) & MASK32
    z = x
    z = ((z ^ (z >> 16)) * 0x85EBCA6B) & MASK32
    z = ((z ^ (z >> 13)) * 0xC2B2AE35) & MASK32
    return x, (z ^ (z >> 16)) & MASK32


def _rotl(x: int, k: int) -> int:
    return ((x << k) | (x >> (32 - k))) & MASK32


class Stream:
    """xoshiro128** seeded by SplitMix32 from (seed, stream id)."""

    def __init__(self, seed: int, stream: int) -> None:
        x = (seed * 0x9E3779B1 + stream * 0x85EBCA77 + 1) & MASK32
        s = []
        for _ in range(4):
            x, out = _splitmix32(x)
            s.append(out)
        if not any(s):
            s[0] = 1
        self.s = s

    def next32(self) -> int:
        s = self.s
        result = (_rotl((s[1] * 5) & MASK32, 7) * 9) & MASK32
        t = (s[1] << 9) & MASK32
        s[2] ^= s[0]
        s[3] ^= s[1]
        s[1] ^= s[2]
        s[0] ^= s[3]
        s[2] ^= t
        s[3] = _rotl(s[3], 11)
        return result

    def uniform(self) -> float:
        """A float in [0, 1) with 53 random bits."""
        a = self.next32() >> 5
        b = self.next32() >> 6
        return (a * 67108864 + b) / 9007199254740992


@dataclass(frozen=True)
class Dist:
    """A non-negative random duration: const, exp or lognormal (by mean)."""

    kind: str
    mean: float
    sigma: float = 0.0

    @staticmethod
    def const(mean: float) -> Dist:
        return Dist("const", mean)

    @staticmethod
    def exp(mean: float) -> Dist:
        return Dist("exp", mean)

    @staticmethod
    def lognormal(mean: float, sigma: float) -> Dist:
        return Dist("lognormal", mean, sigma) if sigma > 0 else Dist.const(mean)

    @staticmethod
    def lognormal_cv(mean: float, cv: float) -> Dist:
        return Dist.lognormal(mean, math.sqrt(math.log(1 + cv * cv)))

    def cv(self) -> float:
        if self.kind == "exp":
            return 1.0
        if self.kind == "lognormal":
            return math.sqrt(math.exp(self.sigma * self.sigma) - 1)
        return 0.0

    def sample(self, rng: Stream) -> float:
        if self.kind == "const":
            return self.mean
        if self.kind == "exp":
            return -self.mean * math.log(1.0 - rng.uniform())
        u1 = 1.0 - rng.uniform()
        u2 = rng.uniform()
        z = math.sqrt(-2.0 * math.log(u1)) * math.cos(2.0 * math.pi * u2)
        mu = math.log(self.mean) - self.sigma * self.sigma / 2.0
        return math.exp(mu + self.sigma * z)


# ------------------------------------------------------------------ model


@dataclass(frozen=True)
class Station:
    name: str
    time: Dist
    mtbf: float | None = None
    mttr: float | None = None
    setup: float = 0.0
    batch: int = 1
    scrap: float = 0.0
    rework: float = 0.0
    rework_to: int | None = None  # None: the station itself


@dataclass(frozen=True)
class Line:
    stations: tuple[Station, ...]
    buffers: tuple[int | None, ...] = ()
    arrivals: Dist | None = None
    conwip: int | None = None

    def __post_init__(self) -> None:
        if len(self.buffers) != len(self.stations) - 1:
            raise ValueError("one buffer between each pair of stations")
        if self.conwip is not None and self.arrivals is not None:
            raise ValueError("CONWIP needs a saturated source")
        for k, s in enumerate(self.stations):
            if s.batch > 1 and (k != 0 or self.arrivals is None):
                raise ValueError("batching only on the first station with arrivals")


# Stream ids: station k uses 10 * k + purpose; arrivals use 9.
PROCESS, FAIL, REPAIR, INSPECT = 1, 2, 3, 4
ARRIVALS = 9

WORKING, STARVED, BLOCKED, DOWN, SETUP = "working", "starved", "blocked", "down", "setup"
STATES = (WORKING, STARVED, BLOCKED, DOWN, SETUP)


@dataclass
class Result:
    horizon: float
    warmup: float
    completed: int = 0
    scrapped: int = 0
    reworked: int = 0
    lead_time_sum: float = 0.0
    wip_area: float = 0.0
    queue_wait_sum: float = 0.0
    queue_wait_n: int = 0
    state_time: list[dict[str, float]] = field(default_factory=list)
    # Traces, when asked for: per station [(t, state)], per buffer [(t, level)].
    states: list[list[tuple[float, str]]] = field(default_factory=list)
    levels: list[list[tuple[float, int]]] = field(default_factory=list)
    wip_trace: list[tuple[float, int]] = field(default_factory=list)

    @property
    def span(self) -> float:
        return self.horizon - self.warmup

    @property
    def throughput(self) -> float:
        """Parts per hour."""
        return self.completed / self.span * 3600

    @property
    def wip(self) -> float:
        return self.wip_area / self.span

    @property
    def lead_time(self) -> float:
        return self.lead_time_sum / self.completed if self.completed else math.nan

    @property
    def queue_wait(self) -> float:
        return self.queue_wait_sum / self.queue_wait_n if self.queue_wait_n else math.nan

    def share(self, k: int, state: str) -> float:
        return self.state_time[k][state] / self.span


class _St:
    __slots__ = ("down", "end", "held", "last_change", "phase", "remaining", "rework", "state", "token")

    def __init__(self) -> None:
        self.held: list[tuple[float, float]] = []  # (birth, start of wait) per part
        self.phase = ""  # "", "setup", "work", "done"
        self.down = False
        self.token = 0  # invalidates scheduled completions
        self.end = 0.0
        self.remaining = 0.0
        self.rework: deque[tuple[float, float]] = deque()
        self.state = STARVED
        self.last_change = 0.0


def simulate(line: Line, horizon: float, seed: int, warmup: float = 0.0, trace: bool = False) -> Result:
    st = line.stations
    n = len(st)
    res = Result(horizon, warmup)
    res.state_time = [dict.fromkeys(STATES, 0.0) for _ in range(n)]
    proc = [Stream(seed, 10 * k + PROCESS) for k in range(n)]
    fail = [Stream(seed, 10 * k + FAIL) for k in range(n)]
    repair = [Stream(seed, 10 * k + REPAIR) for k in range(n)]
    insp = [Stream(seed, 10 * k + INSPECT) for k in range(n)]
    arr = Stream(seed, ARRIVALS)
    s = [_St() for _ in range(n)]
    bufs: list[deque[tuple[float, float]]] = [deque() for _ in range(n - 1)]
    queue: deque[tuple[float, float]] = deque()  # arrivals waiting for station 0
    events: list[tuple[float, int, str, int, int]] = []
    seq = 0
    now = 0.0
    wip = 0
    wip_since = 0.0
    if trace:
        res.states = [[(0.0, STARVED)] for _ in range(n)]
        res.levels = [[(0.0, 0)] for _ in range(n - 1)]
        res.wip_trace = [(0.0, 0)]

    def push(t: float, kind: str, k: int, token: int = 0) -> None:
        nonlocal seq
        heapq.heappush(events, (t, seq, kind, k, token))
        seq += 1

    def in_window(a: float, b: float) -> float:
        return max(0.0, min(b, horizon) - max(a, warmup))

    def set_state(k: int, new: str) -> None:
        x = s[k]
        if new == x.state:
            return
        res.state_time[k][x.state] += in_window(x.last_change, now)
        x.state, x.last_change = new, now
        if trace:
            res.states[k].append((now, new))

    def state_of(k: int) -> str:
        x = s[k]
        if x.down:
            return DOWN
        if x.phase == "setup":
            return SETUP
        if x.phase == "work":
            return WORKING
        if x.phase == "done":
            return BLOCKED
        return STARVED

    def change_wip(d: int) -> None:
        nonlocal wip, wip_since
        res.wip_area += wip * in_window(wip_since, now)
        wip += d
        wip_since = now
        if trace:
            res.wip_trace.append((now, wip))

    def level(b: int) -> None:
        if trace:
            res.levels[b].append((now, len(bufs[b])))

    def start(k: int, parts: list[tuple[float, float]]) -> None:
        x = s[k]
        x.held = parts
        if k == 0 and line.arrivals is not None:
            for _birth, since in parts:
                if now >= warmup:
                    res.queue_wait_sum += now - since
                    res.queue_wait_n += 1
        if st[k].setup > 0:
            x.phase = "setup"
            x.end = now + st[k].setup
        else:
            x.phase = "work"
            x.end = now + sum(st[k].time.sample(proc[k]) for _ in parts)
        x.token += 1
        push(x.end, "end", k, x.token)
        set_state(k, state_of(k))

    def empty(k: int) -> bool:
        return not s[k].held and not s[k].down

    def try_pull(k: int) -> bool:
        x = s[k]
        if x.held or x.down:
            return False
        if x.rework:
            start(k, [x.rework.popleft()])
            return True
        if k > 0:
            if bufs[k - 1]:
                birth, _ = bufs[k - 1].popleft()
                level(k - 1)
                start(k, [(birth, now)])
                return True
            return False
        if line.arrivals is not None:
            b = st[0].batch
            if len(queue) >= b:
                start(0, [queue.popleft() for _ in range(b)])
                return True
            return False
        if line.conwip is not None and wip >= line.conwip:
            return False
        change_wip(+1)
        start(0, [(now, now)])
        return True

    def try_push(k: int) -> bool:
        x = s[k]
        if x.phase != "done" or x.down:
            return False
        if k == n - 1:
            for birth, _ in x.held:
                if now >= warmup and now <= horizon:
                    res.completed += 1
                    res.lead_time_sum += now - birth
                change_wip(-1)
            x.held, x.phase = [], ""
            set_state(k, state_of(k))
            return True
        cap = line.buffers[k]
        if not bufs[k] and empty(k + 1) and not s[k + 1].rework:
            part = x.held[0]
            x.held, x.phase = [], ""
            set_state(k, state_of(k))
            start(k + 1, [(part[0], now)])
            return True
        if cap is None or len(bufs[k]) < cap:
            bufs[k].append(x.held[0])
            level(k)
            x.held, x.phase = [], ""
            set_state(k, state_of(k))
            return True
        return False

    def settle() -> None:
        moved = True
        while moved:
            moved = False
            for k in range(n - 1, -1, -1):
                if try_push(k):
                    moved = True
            for k in range(n):
                if try_pull(k):
                    moved = True

    def finish_work(k: int) -> None:
        x = s[k]
        sp = st[k]
        if sp.scrap > 0 or sp.rework > 0:
            u = insp[k].uniform()
            if u < sp.scrap:
                if now >= warmup and now <= horizon:
                    res.scrapped += 1
                x.held, x.phase = [], ""
                change_wip(-1)
                set_state(k, state_of(k))
                return
            if u < sp.scrap + sp.rework:
                if now >= warmup and now <= horizon:
                    res.reworked += 1
                target = k if sp.rework_to is None else sp.rework_to
                part = x.held[0]
                if target == k:
                    x.phase = "work"
                    x.end = now + sp.time.sample(proc[k])
                    x.token += 1
                    push(x.end, "end", k, x.token)
                    return
                s[target].rework.append((part[0], now))
                x.held, x.phase = [], ""
                set_state(k, state_of(k))
                return
        x.phase = "done"
        set_state(k, state_of(k))

    for k, sp in enumerate(st):
        if sp.mtbf is not None:
            push(-sp.mtbf * math.log(1.0 - fail[k].uniform()), "fail", k)
    if line.arrivals is not None:
        push(line.arrivals.sample(arr), "arrive", 0)
    settle()

    while events:
        t, _, kind, k, token = heapq.heappop(events)
        if t > horizon:
            break
        now = t
        x = s[k]
        if kind == "end":
            if token != x.token or x.down:
                continue
            if x.phase == "setup":
                x.phase = "work"
                x.end = now + sum(st[k].time.sample(proc[k]) for _ in x.held)
                x.token += 1
                push(x.end, "end", k, x.token)
                set_state(k, state_of(k))
            else:
                finish_work(k)
        elif kind == "fail":
            x.down = True
            if x.phase in ("setup", "work"):
                x.remaining = x.end - now
                x.token += 1
            set_state(k, DOWN)
            mttr = st[k].mttr or 0.0
            push(now - mttr * math.log(1.0 - repair[k].uniform()), "repair", k)
        elif kind == "repair":
            x.down = False
            if x.phase in ("setup", "work"):
                x.end = now + x.remaining
                x.token += 1
                push(x.end, "end", k, x.token)
            set_state(k, state_of(k))
            mtbf = st[k].mtbf or 0.0
            push(now - mtbf * math.log(1.0 - fail[k].uniform()), "fail", k)
        elif kind == "arrive":
            change_wip(+1)
            queue.append((now, now))
            push(now + line.arrivals.sample(arr), "arrive", 0)  # type: ignore[union-attr]
        settle()

    now = horizon
    for k in range(n):
        res.state_time[k][s[k].state] += in_window(s[k].last_change, now)
    res.wip_area += wip * in_window(wip_since, now)
    return res


# ------------------------------------------------------------ replications


@dataclass(frozen=True)
class Estimate:
    """Mean of replications with a 95 % confidence interval (Student t)."""

    mean: float
    half: float
    n: int

    def fmt(self, digits: int = 1) -> str:
        return f"{self.mean:.{digits}f} ± {self.half:.{digits}f}"


# Two-sided 95 % quantiles of Student's t, by degrees of freedom.
_T95 = {1: 12.706, 2: 4.303, 3: 3.182, 4: 2.776, 5: 2.571, 6: 2.447, 7: 2.365, 8: 2.306, 9: 2.262,
        10: 2.228, 14: 2.145, 19: 2.093, 24: 2.064, 29: 2.045, 39: 2.023, 49: 2.010, 99: 1.984}


def t95(df: int) -> float:
    if df in _T95:
        return _T95[df]
    if df > 99:
        return 1.96 + 2.4 / df
    lower = max(d for d in _T95 if d < df)
    upper = min(d for d in _T95 if d > df)
    w = (df - lower) / (upper - lower)
    return _T95[lower] * (1 - w) + _T95[upper] * w


def estimate(values: list[float]) -> Estimate:
    n = len(values)
    m = sum(values) / n
    if n < 2:
        return Estimate(m, math.nan, n)
    var = sum((v - m) ** 2 for v in values) / (n - 1)
    return Estimate(m, t95(n - 1) * math.sqrt(var / n), n)


def replicate(line: Line, horizon: float, reps: int, warmup: float = 0.0, seed0: int = 1) -> list[Result]:
    return [simulate(line, horizon, seed0 + r, warmup) for r in range(reps)]
