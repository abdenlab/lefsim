---
title: Capturing contacts
view: contacts
lattice: 3000
lefs: 20
ctcfs: 750 >, 1500 >, 2249 <
strength: 0.9
speed: 300
seed: 5
---

A molecular technique called Hi-C measures how often pairs of sites in the genome touch, averaged over
millions of cells, producing a contact frequency map.

We can give a crude estimate of 3D contact probabilities for every pair of sites from our 1D model, shown in this heatmap. 

Loops bring sites that are far apart along the fiber close
together, so they touch more often. Above the diagonal shows the current moment, and below we average over time.
