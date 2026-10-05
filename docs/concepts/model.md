# The LEF model

A loop-extrusion factor is a protein complex that binds a chromatin polymer and
extrudes a loop by walking two legs outward from a shared binding site. CTCF
sites act as boundaries, depending on which side a leg meets them from.

## The world

- The **lattice** is a fixed 1D array of sites. Most are plain; some are
  CTCFs, each with a polarity (`captures_from`, its capture side) and a
  `strength` in [0, 1]. The lattice never changes during a run.
- A **LEF** has two **legs** that face outward from where it bound: the left
  leg steps left, the right leg steps right, and neither steps backward.
- A leg's **mobility** is `Moving` or `Captured` (held by a CTCF).
- A moving leg is **blocked** while another leg sits on the next site ahead, or
  the lattice ends there. Blocking is derived from positions, never stored, so
  it lifts as soon as the obstruction moves.
- A leg is **protected** while its partner is captured: it's immune to
  eviction.

## Actions and their rates

Three kinds of action compete. Each fires after an exponential wait at its
current rate; `k` below is the step rate, `rates.step`.

**Step** (per leg): the leg moves one site in the direction it faces.

- Scheduled at rate `k` for a moving, unblocked leg.
- At a CTCF met from behind (see below), its rate is reduced, or it can't
  step at all if a captured leg holds that CTCF.

**Evict** (per leg): a leg meeting a CTCF from behind is turned back by it, and
its LEF unbinds at once.

**Respawn** (per LEF): the LEF unbinds and rebinds at a site chosen by the
spawn rule, both legs on that one site. It fires at the first rate that
applies:

1. `rates.respawn_captured` if either leg is captured: capture protects the
   LEF;
2. `rates.respawn_blocked` if either leg is blocked;
3. `rates.respawn`, the basal rate, otherwise.

## CTCFs

A CTCF's `captures_from` names its capture side; the other side evicts.

- `captures_from: Right`: its right side captures, its left side evicts.
- `captures_from: Left`: its left side captures, its right side evicts.

**Capture.** A leg stepping onto a CTCF from its capture side is captured with
probability `strength`, else it passes through, still moving. A captured leg
stays until its LEF respawns.

**Eviction.** A leg facing a CTCF from its other side races two clocks:

| The CTCF | Step | Evict |
|---|---|---|
| Empty | `(1 − strength) · k`: passes onto it, moving | `strength · k` |
| Holds a captured leg | never | `k`: it's certainly occupied |
| Holds a moving leg | never (traffic) | never |

Two independent clocks at `(1 − p)·k` and `p·k` are exactly a step at `k`
followed by a coin flip with probability `p`, so `strength` crudely models how
often the CTCF is occupied. Eviction unbinds the LEF in the same event, so no
transient state is ever stored.

**Protection.** A leg whose partner is captured ignores a CTCF's rear side: it
steps onto an empty one at the full rate `k`, and a held one blocks it, as
traffic. So once a LEF is held at one boundary, its free leg can extrude
through the next one if it slips past the CTCF that would capture it.

## Spawning

A spawn rule picks where a LEF rebinds. The built-in rule,
`UniformNonCtcf`, rejection-samples until it finds an empty plain site (it
needs 2n − 1 plain sites for n LEFs). Custom rules implement the small `Spawn`
trait. Binding is not a step: a LEF landing on a CTCF may be captured there,
but never evicted. `populate` places a model's starting LEFs with the same
rule, from its own seed.

## Validity

A world is checked as it's built, so it's always a state the dynamics could
reach: legs on the lattice and not crossed, no two LEFs on one site (a LEF's own
two legs may share one), and a captured leg only on a CTCF that could have
captured it. A model also checks that its rates are positive and finite, and
that its spawn rule can always place its LEFs.
