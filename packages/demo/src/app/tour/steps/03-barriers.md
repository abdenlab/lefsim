---
title: "Barriers: CTCF"
view: kymograph
lattice: 3000
lefs: 8
ctcfs: 1000 >, 2000 <
strength: 0.9
speed: 100
seed: 4
---
Extruders don't move freely forever. The protein **CTCF** binds specific sites
on the DNA, and those sites have a direction, drawn as chevrons in the lanes
under the loops.

A leg arriving at a CTCF from the side it faces is caught and **held** there,
and its loop turns orange. A leg arriving from behind may knock its extruder off
the fiber instead. While one leg is held, its partner is safe from that, so the
loop can keep growing on its other side.

The strength of a CTCF is the chance that it acts on a leg at all; otherwise the
leg passes by.

*Try it:* drag a CTCF to move it, or click one to flip it around.
