---
title: "Barriers"
view: kymograph
lattice: 3000
lefs: 8
ctcfs: 1000 >, 2000 <
strength: 0.9
speed: 100
seed: 4
---
The protein [CTCF](https://en.wikipedia.org/wiki/CTCF) binds specific sites
on the DNA, and those sites have an orientation, drawn as chevrons in the two lanes under the loops.

Extruders don't move freely forever. A leg arriving at a CTCF site from its capture side is caught and **held** there. This is depicted by its loop turning orange. The "strength" of a CTCF site is the chance that it acts on a leg; otherwise the leg passes by.

A LEF whose leg arrives at a CTCF site from the other side may get knocked off instead. It is protected if its partner leg is held by another CTCF, allowing the loop to keep growing.

*Try it:* drag a CTCF site to move it, or click on one to flip it around.
