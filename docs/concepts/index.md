# Concepts: how production lines behave

This learning path teaches the ideas behind production-line simulation,
from the ground up.

## Who this is for

Developers who know code and some statistics, but have never worked in a
factory. You do not need to know what a buffer, a
bottleneck or takt time is; every term is defined where it first appears and
collected in the glossary.

## What you will be able to do afterwards

- Read a production line the way an engineer does: find its bottleneck,
  estimate its output, and say where work will pile up.
- Explain why a line misses the output that simple arithmetic promises, and
  which of variability, failures, setups or scrap is to blame.
- Predict what a change will do, such as a bigger buffer, a faster machine
  or a limit on work in progress, and check the prediction with a simulation.
- Judge a simulation result: how many runs it needs, how to read its
  confidence interval, and when a model can be trusted to describe a real
  line.

## The field in one paragraph

A production line turns raw material into products by sending it through a
sequence of stations, with storage in between. Its performance comes down to
a few quantities: how much it produces per hour (throughput), how much
unfinished work sits in it (work in progress) and how long one part takes to
get through (lead time). A handful of laws tie these together. The slowest
station limits the output, and the three quantities are linked by Little's
Law. What makes real lines hard is variability: process times that vary,
machines that fail, setups and scrap. Variability makes stations wait for
each other, and buffers and work-in-progress limits are the tools to manage
that. Queueing theory gives exact answers for small, simple cases;
simulation covers the rest, and its answers need confidence intervals.

## The learning path

Each page builds on the ones before it. One example line runs through all of
them: it starts with two stations and grows a little on each page.

1. **What a production system is:** stations, buffers and flow, and the
   four numbers that describe a line: throughput, work in progress, cycle
   time and lead time.
2. **Capacity and the bottleneck:** why, in a line without randomness, the
   slowest station sets the pace for all the others.
3. **Little's Law:** the one relation between work in progress, throughput
   and lead time that holds in almost any system.
4. **Variability:** where randomness in a line comes from and how to
   measure it with the coefficient of variation.
5. **Utilization and waiting:** why waiting times explode as a station
   gets close to 100 % busy, and what Kingman's formula says about it.
6. **Starving and blocking:** how a station has to wait when its neighbour
   is too slow or too full.
7. **Buffers:** how storage between stations decouples them, and why each
   extra place helps less than the one before.
8. **Unreliable machines:** failures and repairs, availability, and why
   rare long breakdowns hurt more than frequent short ones.
9. **Longer lines:** how disturbances travel along many stations, why the
   bottleneck can move, and why more buffer everywhere is not the answer.
10. **Takt time and pull:** pacing a line to customer demand, push versus
    pull, and limiting work in progress with Kanban and CONWIP.
11. **Batching and setups:** the trade-off between losing capacity to setups
    and losing time to large batches.
12. **Scrap and rework:** how inspection, yield and rework loops reduce what
    a line really delivers.
13. **Simulation you can trust:** replications, confidence intervals,
    warm-up, and how many runs are enough.
14. **From model to reality:** collecting data, fitting distributions, and
    checking a model against the real line.

The pages are being written; until they appear in the sidebar, the
[specification](../spec.md) describes what taktsim will do.
