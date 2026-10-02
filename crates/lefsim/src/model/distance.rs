//! Effective distance: the shortest path between lattice sites when a path
//! may walk the lattice and also hop across LEFs.
//!
//! A path walks at `step_length` per lattice step and crosses a LEF, from one
//! leg to the other, at `hop_length`. A **rosette** (a chain of LEFs side by
//! side, one's right leg next to the next's left leg) costs one hop per LEF
//! crossed, and its junctions are free: crossing `k` linked LEFs costs
//! `k * hop_length`, without the steps between them. Both lengths are
//! integers, so distances are exact.
//!
//! Use `hop_length >= step_length`. With shorter hops, reversing back across
//! a LEF can beat a single step (e.g. four hops around a rosette's junction
//! instead of one step), which isn't intended; forbidding it is sketched in
//! the design note but not implemented.
//!
//! [`LoopGraph`] contracts the lattice to the leg sites: consecutive legs are
//! joined by the walk between them, and each rosette gets two one-way
//! "express lanes" that carry a path across its LEFs (from a left leg to any
//! right leg at or after it along the chain, and back), a hop per LEF. A site
//! enters the network through its nearest leg on each side. See
//! `_scratch/design-effective-distance.md` for the reasoning.

use std::cmp::Reverse;
use std::collections::BinaryHeap;

use super::lattice::Position;
use super::world::World;

/// Why an effective distance can't be computed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DistanceError {
    /// The step length must be positive.
    ZeroStepLength,
    SiteOutOfRange { site: Position, n_sites: usize },
}

impl std::fmt::Display for DistanceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DistanceError::ZeroStepLength => write!(f, "the step length must be positive"),
            DistanceError::SiteOutOfRange { site, n_sites } => {
                write!(f, "site {site} is outside a lattice of {n_sites} sites")
            }
        }
    }
}

impl std::error::Error for DistanceError {}

const UNREACHED: u64 = u64::MAX;

/// The loop network of one state, ready for effective-distance queries.
///
/// Built once per state in `O(n log n)` for `n` legs; queries don't change
/// it. Nodes `0 .. n` are the legs in position order; then two lane nodes
/// per LEF (rightward, leftward). Edges are stored compactly (CSR).
#[derive(Clone, Debug)]
pub struct LoopGraph {
    n_sites: usize,
    step: u64,
    /// Position of each leg node, in sorted order.
    pos: Vec<Position>,
    /// Outgoing edges of node `v`: `targets[offsets[v]..offsets[v + 1]]`.
    offsets: Vec<usize>,
    targets: Vec<u32>,
    lengths: Vec<u64>,
}

impl LoopGraph {
    /// The loop network of `world`'s current state.
    pub fn new(world: &World, step_length: u64, hop_length: u64) -> Result<Self, DistanceError> {
        let lefs = world.lef_states();
        Self::from_legs(world.lattice().len(), &lefs.left_pos, &lefs.right_pos, step_length, hop_length)
    }

    /// The loop network of LEFs with legs at `left_pos[i]` and
    /// `right_pos[i]` on a lattice of `n_sites` (e.g. one recorded snapshot).
    /// Positions are taken as given: legs off the lattice are an error only
    /// when a query reaches them.
    pub fn from_legs(
        n_sites: usize,
        left_pos: &[Position],
        right_pos: &[Position],
        step_length: u64,
        hop_length: u64,
    ) -> Result<Self, DistanceError> {
        if step_length == 0 {
            return Err(DistanceError::ZeroStepLength);
        }
        let n_lefs = left_pos.len().min(right_pos.len());
        let n_legs = 2 * n_lefs;

        // Legs in position order; at one site (a freshly placed LEF), the left
        // leg first. `leg = 2 * lef + side`, side 0 left, 1 right.
        let mut order: Vec<u32> = (0..n_legs as u32).collect();
        let leg_pos = |leg: u32| if leg.is_multiple_of(2) { left_pos[leg as usize / 2] } else { right_pos[leg as usize / 2] };
        order.sort_by_key(|&leg| (leg_pos(leg), leg % 2));
        let pos: Vec<Position> = order.iter().map(|&leg| leg_pos(leg)).collect();
        let mut node_of = vec![0u32; n_legs];
        for (node, &leg) in order.iter().enumerate() {
            node_of[leg as usize] = node as u32;
        }
        let left_node = |lef: usize| node_of[2 * lef];
        let right_node = |lef: usize| node_of[2 * lef + 1];

        // Links: a right leg at p followed by another LEF's left leg at p + 1.
        let mut next = vec![u32::MAX; n_lefs];
        let mut prev = vec![u32::MAX; n_lefs];
        for k in 0..n_legs.saturating_sub(1) {
            let (a, b) = (order[k], order[k + 1]);
            if a % 2 == 1 && b % 2 == 0 && a / 2 != b / 2 && pos[k + 1] == pos[k] + 1 {
                next[a as usize / 2] = b / 2;
                prev[b as usize / 2] = a / 2;
            }
        }

        // Edges, as (from, to, length). Lane nodes: rightward R_i = n_legs +
        // 2i, leftward L_i = n_legs + 2i + 1.
        let rightward = |lef: usize| (n_legs + 2 * lef) as u32;
        let leftward = |lef: usize| (n_legs + 2 * lef + 1) as u32;
        let mut edges: Vec<(u32, u32, u64)> = Vec::with_capacity(2 * n_legs + 6 * n_lefs);
        for k in 0..n_legs.saturating_sub(1) {
            let walk = step_length * u64::from(pos[k + 1] - pos[k]);
            edges.push((k as u32, k as u32 + 1, walk));
            edges.push((k as u32 + 1, k as u32, walk));
        }
        for lef in 0..n_lefs {
            // Rightward: enter from the left leg (a hop across this LEF), move
            // on along the chain (a hop across each further LEF, the junction
            // free), leave at a right leg.
            edges.push((left_node(lef), rightward(lef), hop_length));
            edges.push((rightward(lef), right_node(lef), 0));
            if next[lef] != u32::MAX {
                edges.push((rightward(lef), rightward(next[lef] as usize), hop_length));
            }
            // Leftward: the mirror image.
            edges.push((right_node(lef), leftward(lef), hop_length));
            edges.push((leftward(lef), left_node(lef), 0));
            if prev[lef] != u32::MAX {
                edges.push((leftward(lef), leftward(prev[lef] as usize), hop_length));
            }
        }

        let n_nodes = n_legs + 2 * n_lefs;
        let mut offsets = vec![0usize; n_nodes + 1];
        for &(from, _, _) in &edges {
            offsets[from as usize + 1] += 1;
        }
        for v in 0..n_nodes {
            offsets[v + 1] += offsets[v];
        }
        let mut fill = offsets.clone();
        let mut targets = vec![0u32; edges.len()];
        let mut lengths = vec![0u64; edges.len()];
        for (from, to, length) in edges {
            let at = &mut fill[from as usize];
            targets[*at] = to;
            lengths[*at] = length;
            *at += 1;
        }
        Ok(LoopGraph { n_sites, step: step_length, pos, offsets, targets, lengths })
    }

    fn check(&self, site: Position) -> Result<(), DistanceError> {
        if site as usize >= self.n_sites {
            return Err(DistanceError::SiteOutOfRange { site, n_sites: self.n_sites });
        }
        Ok(())
    }

    /// Walking between two sites.
    fn walk(&self, a: Position, b: Position) -> u64 {
        self.step * u64::from(a.abs_diff(b))
    }

    /// The (up to two) entry legs of `site`: the nearest leg at or left of
    /// it, and the nearest at or right of it.
    fn entries(&self, site: Position) -> ([usize; 2], usize) {
        let right = self.pos.partition_point(|&p| p < site);
        let at_or_left = self.pos.partition_point(|&p| p <= site);
        let mut out = [0; 2];
        let mut n = 0;
        if at_or_left > 0 {
            out[n] = at_or_left - 1;
            n += 1;
        }
        if right < self.pos.len() && (n == 0 || out[0] != right) {
            out[n] = right;
            n += 1;
        }
        (out, n)
    }

    /// Dijkstra from `seeds` (node, starting distance). With `stop`, returns
    /// once all of those nodes are settled.
    fn dijkstra(&self, seeds: &[(usize, u64)], stop: &[usize]) -> Vec<u64> {
        let mut dist = vec![UNREACHED; self.offsets.len() - 1];
        let mut heap = BinaryHeap::new();
        for &(node, d) in seeds {
            if d < dist[node] {
                dist[node] = d;
                heap.push(Reverse((d, node as u32)));
            }
        }
        let mut remaining = stop.len();
        let mut settled = vec![false; dist.len()];
        while let Some(Reverse((d, v))) = heap.pop() {
            let v = v as usize;
            if settled[v] || d > dist[v] {
                continue;
            }
            settled[v] = true;
            if remaining > 0 && stop.contains(&v) {
                remaining -= 1;
                if remaining == 0 {
                    break;
                }
            }
            for e in self.offsets[v]..self.offsets[v + 1] {
                let (w, nd) = (self.targets[e] as usize, d + self.lengths[e]);
                if nd < dist[w] {
                    dist[w] = nd;
                    heap.push(Reverse((nd, w as u32)));
                }
            }
        }
        dist
    }

    /// From a table of distances to every node, the distance to `site`:
    /// through its best entry leg.
    fn via_entries(&self, dist: &[u64], site: Position) -> u64 {
        let (entries, n) = self.entries(site);
        entries[..n]
            .iter()
            .map(|&e| dist[e].saturating_add(self.walk(self.pos[e], site)))
            .min()
            .unwrap_or(UNREACHED)
    }

    fn seeds(&self, site: Position) -> Vec<(usize, u64)> {
        let (entries, n) = self.entries(site);
        entries[..n].iter().map(|&e| (e, self.walk(site, self.pos[e]))).collect()
    }

    /// The effective distance between sites `a` and `b`.
    pub fn distance(&self, a: Position, b: Position) -> Result<u64, DistanceError> {
        self.check(a)?;
        self.check(b)?;
        if self.pos.is_empty() {
            return Ok(self.walk(a, b));
        }
        let (stop, n) = self.entries(b);
        let dist = self.dijkstra(&self.seeds(a), &stop[..n]);
        Ok(self.walk(a, b).min(self.via_entries(&dist, b)))
    }

    /// The effective distance from `a` to each of `targets`: one Dijkstra.
    pub fn distances_from(&self, a: Position, targets: &[Position]) -> Result<Vec<u64>, DistanceError> {
        self.check(a)?;
        targets.iter().try_for_each(|&t| self.check(t))?;
        if self.pos.is_empty() {
            return Ok(targets.iter().map(|&t| self.walk(a, t)).collect());
        }
        let dist = self.dijkstra(&self.seeds(a), &[]);
        Ok(targets.iter().map(|&t| self.walk(a, t).min(self.via_entries(&dist, t))).collect())
    }

    /// The effective distance from each of `sites` to each of `targets`,
    /// row-major (`sites.len()` rows). Sites sharing an entry leg share its
    /// Dijkstra, so it takes one run per distinct entry leg of `sites`.
    pub fn distances(&self, sites: &[Position], targets: &[Position]) -> Result<Vec<u64>, DistanceError> {
        sites.iter().chain(targets).try_for_each(|&s| self.check(s))?;
        let m = targets.len();
        let mut out = Vec::with_capacity(sites.len() * m);
        for &a in sites {
            out.extend(targets.iter().map(|&t| self.walk(a, t)));
        }
        if self.pos.is_empty() || m == 0 {
            return Ok(out);
        }
        // Which sites enter through each leg.
        let mut by_entry: Vec<Vec<usize>> = vec![Vec::new(); self.pos.len()];
        for (i, &a) in sites.iter().enumerate() {
            let (entries, n) = self.entries(a);
            for &e in &entries[..n] {
                by_entry[e].push(i);
            }
        }
        let mut to_target = vec![0u64; m];
        for (e, users) in by_entry.iter().enumerate() {
            if users.is_empty() {
                continue;
            }
            let dist = self.dijkstra(&[(e, 0)], &[]);
            for (j, &t) in targets.iter().enumerate() {
                to_target[j] = self.via_entries(&dist, t);
            }
            for &i in users {
                let lead = self.walk(sites[i], self.pos[e]);
                let row = &mut out[i * m..(i + 1) * m];
                for (cell, &rest) in row.iter_mut().zip(&to_target) {
                    *cell = (*cell).min(lead.saturating_add(rest));
                }
            }
        }
        Ok(out)
    }

    /// The effective distance of every pair of `sites`, condensed: entry
    /// `(i, j)` for `i < j`, in the order of `scipy.spatial.distance.pdist`.
    pub fn pairwise(&self, sites: &[Position]) -> Result<Vec<u64>, DistanceError> {
        let k = sites.len();
        let full = self.distances(sites, sites)?;
        let mut out = Vec::with_capacity(k * k.saturating_sub(1) / 2);
        for i in 0..k {
            out.extend_from_slice(&full[i * k + i + 1..(i + 1) * k]);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use rand::{Rng, SeedableRng};

    use super::*;
    use crate::model::{Lattice, Mobility, SimRng};

    /// Every pair's distance by brute force: the full site graph (steps of
    /// `s`), plus an explicit edge for every chain crossing (`l_i` to `r_j`
    /// for `i <= j` along each rosette, `h` per LEF crossed), all-pairs by
    /// Floyd–Warshall. Independent of the lanes, so it checks them.
    fn brute_force(n_sites: usize, lefs: &[(u32, u32)], s: u64, h: u64) -> Vec<Vec<u64>> {
        let inf = u64::MAX / 4;
        let mut d = vec![vec![inf; n_sites]; n_sites];
        for (i, row) in d.iter_mut().enumerate() {
            row[i] = 0;
            if i + 1 < n_sites {
                row[i + 1] = s;
            }
            if i > 0 {
                row[i - 1] = s;
            }
        }
        // Chains: follow links (right leg at p, another LEF's left leg at p + 1).
        let next = |a: usize| (0..lefs.len()).find(|&b| b != a && lefs[b].0 == lefs[a].1 + 1);
        let has_prev = |b: usize| (0..lefs.len()).any(|a| a != b && lefs[b].0 == lefs[a].1 + 1);
        for start in (0..lefs.len()).filter(|&a| !has_prev(a)) {
            let mut chain = vec![start];
            while let Some(b) = next(*chain.last().unwrap()) {
                chain.push(b);
            }
            for (x, &i) in chain.iter().enumerate() {
                for (y, &j) in chain.iter().enumerate().skip(x) {
                    let (l, r) = (lefs[i].0 as usize, lefs[j].1 as usize);
                    let hops = h * (y - x + 1) as u64;
                    d[l][r] = d[l][r].min(hops);
                    d[r][l] = d[r][l].min(hops);
                }
            }
        }
        for k in 0..n_sites {
            for i in 0..n_sites {
                for j in 0..n_sites {
                    let via = d[i][k] + d[k][j];
                    if via < d[i][j] {
                        d[i][j] = via;
                    }
                }
            }
        }
        d
    }

    fn graph(n_sites: usize, lefs: &[(u32, u32)], s: u64, h: u64) -> LoopGraph {
        let (l, r): (Vec<u32>, Vec<u32>) = lefs.iter().copied().unzip();
        LoopGraph::from_legs(n_sites, &l, &r, s, h).unwrap()
    }

    /// Random LEFs on distinct sites (crossing allowed), with a bias toward
    /// side-by-side placements so rosettes are common.
    fn random_lefs(rng: &mut SimRng, n_sites: u32, n_lefs: usize) -> Vec<(u32, u32)> {
        let mut used = vec![false; n_sites as usize];
        let mut lefs = Vec::new();
        for _ in 0..n_lefs * 10 {
            if lefs.len() == n_lefs {
                break;
            }
            let (l, r) = if !lefs.is_empty() && rng.gen_bool(0.5) {
                // Next to an existing right leg: a link.
                let &(_, pr): &(u32, u32) = &lefs[rng.gen_range(0..lefs.len())];
                let l = pr + 1;
                (l, l + rng.gen_range(0..6))
            } else {
                let l = rng.gen_range(0..n_sites);
                (l, l + rng.gen_range(0..10))
            };
            if r >= n_sites || used[l as usize] || used[r as usize] {
                continue;
            }
            used[l as usize] = true;
            used[r as usize] = true;
            lefs.push((l, r));
        }
        lefs
    }

    #[test]
    fn matches_brute_force_on_random_worlds() {
        let mut rng = SimRng::seed_from_u64(7);
        for trial in 0..200 {
            let n_sites = rng.gen_range(5..40);
            let n_lefs = rng.gen_range(0..8);
            let lefs = random_lefs(&mut rng, n_sites, n_lefs);
            for &(s, h) in &[(1, 0), (1, 1), (1, 5), (1000, 100), (3, 7)] {
                let expected = brute_force(n_sites as usize, &lefs, s, h);
                let g = graph(n_sites as usize, &lefs, s, h);
                let sites: Vec<u32> = (0..n_sites).collect();
                let all = g.distances(&sites, &sites).unwrap();
                for a in 0..n_sites as usize {
                    for b in 0..n_sites as usize {
                        let got = all[a * n_sites as usize + b];
                        assert_eq!(got, expected[a][b], "trial {trial}, s={s}, h={h}, {a}->{b}, lefs {lefs:?}");
                    }
                }
                // The single-query paths agree with the batched one.
                let (a, b) = (rng.gen_range(0..n_sites), rng.gen_range(0..n_sites));
                assert_eq!(g.distance(a, b).unwrap(), expected[a as usize][b as usize]);
                let from = g.distances_from(a, &sites).unwrap();
                assert_eq!(from, all[a as usize * n_sites as usize..(a as usize + 1) * n_sites as usize]);
            }
        }
    }

    #[test]
    fn no_lefs_is_the_plain_walk() {
        let g = graph(100, &[], 3, 1);
        assert_eq!(g.distance(10, 60).unwrap(), 150);
        assert_eq!(g.pairwise(&[0, 5, 9]).unwrap(), vec![15, 27, 12]);
    }

    #[test]
    fn the_closest_entry_leg_is_not_always_the_way_in() {
        // LEFs at (0, 1000) and (8, 20): from 5 to 1000, go left to leg 0.
        let g = graph(1001, &[(0, 1000), (8, 20)], 1, 1);
        assert_eq!(g.distance(5, 1000).unwrap(), 5 + 1);
    }

    #[test]
    fn a_rosette_takes_a_hop_per_lef_and_its_junctions_are_free() {
        let (s, h) = (10, 30);
        for k in 1..6u32 {
            // k LEFs side by side: [1, 10], [11, 20], ...
            let lefs: Vec<(u32, u32)> = (0..k).map(|i| (10 * i + 1, 10 * i + 10)).collect();
            let g = graph(10 * k as usize + 2, &lefs, s, h);
            assert_eq!(g.distance(1, 10 * k).unwrap(), u64::from(k) * h, "k = {k}");
        }
        // Not linked (two sites apart), the step between is paid.
        let g = graph(40, &[(1, 10), (12, 20)], s, h);
        assert_eq!(g.distance(1, 20).unwrap(), 2 * h + 2 * s);
    }

    #[test]
    fn the_junction_rules() {
        // LEF 1 at 10-20, LEF 2 at 21-30, as in the design's table: with
        // hops at least as long as steps, s, s + h, 2h and h + s. When a hop
        // is shorter than a step, routes with extra hops win: r1 to l2 can go
        // back across LEF 1, over the chain, and back across LEF 2.
        for &(s, h) in &[(1, 1), (1, 5), (3, 7), (1, 0), (1000, 100)] {
            let g = graph(40, &[(10, 20), (21, 30)], s, h);
            assert_eq!(g.distance(20, 21).unwrap(), s.min(4 * h), "r1 to l2");
            assert_eq!(g.distance(20, 30).unwrap(), (s + h).min(3 * h).min(10 * s), "r1 to r2");
            assert_eq!(g.distance(10, 30).unwrap(), (2 * h).min(20 * s), "l1 to r2");
            assert_eq!(g.distance(10, 21).unwrap(), (h + s).min(3 * h).min(11 * s), "l1 to l2");
        }
        // Two sites apart: no link, two hops and the walk between.
        let g = graph(40, &[(10, 20), (22, 30)], 1, 1);
        assert_eq!(g.distance(10, 30).unwrap(), 1 + 2 + 1);
        // A right leg next to a left leg of the same LEF (a two-site loop)
        // is just a LEF, not a link.
        let g = graph(40, &[(10, 11)], 1, 5);
        assert_eq!(g.distance(10, 11).unwrap(), 1);
    }

    #[test]
    fn a_freshly_placed_lef_changes_nothing_by_itself() {
        let g = graph(50, &[(20, 20)], 1, 0);
        assert_eq!(g.distance(10, 30).unwrap(), 20);
    }

    #[test]
    fn built_from_a_world() {
        let lattice = Lattice::plain(100).unwrap();
        let mut world = World::new(lattice);
        world.add_lef((10, Mobility::Moving), (60, Mobility::Moving)).unwrap();
        let g = LoopGraph::new(&world, 2, 1).unwrap();
        assert_eq!(g.distance(5, 65).unwrap(), 2 * 5 + 1 + 2 * 5);
        assert_eq!(LoopGraph::new(&world, 0, 1).unwrap_err(), DistanceError::ZeroStepLength);
        assert_eq!(g.distance(5, 100), Err(DistanceError::SiteOutOfRange { site: 100, n_sites: 100 }));
    }

    #[test]
    fn pairwise_is_the_upper_triangle() {
        let g = graph(60, &[(5, 25), (26, 40), (30, 35)], 2, 1);
        let sites = [3, 27, 41, 12, 36];
        let full = g.distances(&sites, &sites).unwrap();
        let condensed = g.pairwise(&sites).unwrap();
        let mut k = 0;
        for i in 0..sites.len() {
            for j in i + 1..sites.len() {
                assert_eq!(condensed[k], full[i * sites.len() + j]);
                assert_eq!(full[i * sites.len() + j], full[j * sites.len() + i], "symmetric");
                k += 1;
            }
        }
    }
}
