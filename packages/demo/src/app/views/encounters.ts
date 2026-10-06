// Encounter statistics, independent of the UI.
//
// Two sites are in an encounter while their effective distance is at most a
// threshold. Distances are sampled every `dt` of simulation time (not once per
// frame), so durations don't depend on the playback speed, and are resolved to
// `dt`.

/** Histogram bins: durations from `dt` up, each bin twice as wide as the last. */
export const N_BINS = 16;

/** Encounters of one pair of sites. */
export class PairEncounters {
  readonly dt: number;
  threshold: number;
  /** The latest sample, and whether the pair is in an encounter (and since when). */
  distance = 0;
  inEncounter = false;
  #start = 0;
  /** Finished encounters by duration bin, and totals since the last reset. */
  counts = new Float64Array(N_BINS);
  nEncounters = 0;
  totalDuration = 0;
  observedTime = 0;
  encounterTime = 0;

  constructor(dt: number, threshold: number) {
    this.dt = dt;
    this.threshold = threshold;
  }

  /** Start afresh from a first sample at `time`. */
  reset(time: number, distance: number) {
    this.counts = new Float64Array(N_BINS);
    this.nEncounters = 0;
    this.totalDuration = 0;
    this.observedTime = 0;
    this.encounterTime = 0;
    this.distance = distance;
    this.inEncounter = distance <= this.threshold;
    this.#start = time;
  }

  /** The histogram bin of a duration: [1, 2), [2, 4), … in units of `dt`. */
  binOf(duration: number): number {
    return Math.max(0, Math.min(N_BINS - 1, Math.floor(Math.log2(duration / this.dt))));
  }

  /** Record a sample at `time`. */
  sample(time: number, distance: number) {
    this.distance = distance;
    const now = distance <= this.threshold;
    this.observedTime += this.dt;
    if (now) this.encounterTime += this.dt;
    if (now && !this.inEncounter) this.#start = time;
    if (!now && this.inEncounter) {
      // The encounter ended at this sample: it lasted from its first sample to here.
      const duration = time - this.#start;
      this.counts[this.binOf(duration)]!++;
      this.nEncounters++;
      this.totalDuration += duration;
    }
    this.inEncounter = now;
  }
}

/**
 * Encounters of every pair of bins: per pair (condensed upper triangle),
 * whether it's always within the threshold along the lattice, whether it's in
 * an encounter now, encounters started, and samples spent in encounters.
 */
export class MapEncounters {
  readonly dt: number;
  threshold: number;
  centers = new Uint32Array(0);
  trivial = new Uint8Array(0);
  inEncounter = new Uint8Array(0);
  starts = new Float64Array(0);
  samplesIn = new Float64Array(0);
  nSamples = 0;

  constructor(dt: number, threshold: number) {
    this.dt = dt;
    this.threshold = threshold;
  }

  get k(): number {
    return this.centers.length;
  }

  get observedTime(): number {
    return this.nSamples * this.dt;
  }

  setBins(nSites: number, bin: number) {
    const k = Math.ceil(nSites / bin);
    this.centers = Uint32Array.from({ length: k }, (_, i) => Math.min(nSites - 1, Math.floor(i * bin + bin / 2)));
    this.reset();
  }

  reset() {
    const k = this.k;
    const n = (k * (k - 1)) / 2;
    this.trivial = new Uint8Array(n);
    this.inEncounter = new Uint8Array(n);
    this.starts = new Float64Array(n);
    this.samplesIn = new Float64Array(n);
    let at = 0;
    for (let i = 0; i < k; i++)
      for (let j = i + 1; j < k; j++)
        this.trivial[at++] = this.centers[j]! - this.centers[i]! <= this.threshold ? 1 : 0;
    this.nSamples = 0;
  }

  /** One sample of every pair's distance (condensed, as `effectiveDistances` returns them). */
  sample(dist: ArrayLike<number>) {
    const thr = this.threshold;
    for (let at = 0; at < dist.length; at++) {
      if (dist[at]! <= thr) {
        this.samplesIn[at]!++;
        if (!this.inEncounter[at]) {
          this.starts[at]!++;
          this.inEncounter[at] = 1;
        }
      } else {
        this.inEncounter[at] = 0;
      }
    }
    this.nSamples++;
  }

  index(i: number, j: number): number {
    return i * this.k - (i * (i + 1)) / 2 + (j - i - 1);
  }
}
