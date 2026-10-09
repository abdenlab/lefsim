---
title: Many at once
view: kymograph
lattice: 3000
lefs: 30
ctcfs: none
speed: 100
seed: 2
---
A chromosome carries many extruders at once, and they get in each other's way. We don't yet fully understand all the traffic rules LEFs follow: this app shows just one possible model.

In this particular model, a leg can't step onto a site that another leg holds or cross another leg, so it waits **blocked**. Loops end up side by side or nested. In the kymograph you can see legs stall where they meet.

In this setup, blocked extruders let go sooner than free ones, which helps keeps the traffic moving.
