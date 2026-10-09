// The tour: scenes written as Markdown files in ./scenes, each opening with a
// settings block that sets up its simulation. The files are bundled at build time.
//
// A scene file looks like this:
//
//   ---
//   title: Barriers
//   view: kymograph
//   lattice: 3000
//   lefs: 6
//   ctcfs: 1000 >, 2000 <
//   strength: 0.9
//   speed: 100
//   seed: 2
//   ---
//   The text, in Markdown.
//
// `ctcfs` lists sites with the way each CTCF faces: `>` captures legs arriving
// from the right, `<` from the left (the chevrons on the stage point the same
// way). Leave it out, or write `none`, for no CTCFs. The rates are `step`,
// `unload`, `unload_blocked` and `unload_held`. `title` and `view` are
// required; anything else left out takes the app's default. Lines starting
// with # are comments.

import { marked } from "marked";

import { DEFAULT_N_SITES, DEFAULT_RATES, DEFAULT_STRENGTH, type Preset } from "../run.svelte.ts";

export interface TourScene {
  /** The file name, without its extension (e.g. "01-loop-extruder"). */
  id: string;
  title: string;
  /** The view tab to show (its key, e.g. "kymograph"). */
  view: string;
  preset: Preset;
  /** The text, as HTML. */
  html: string;
}

const files = import.meta.glob("./scenes/*.md", { query: "?raw", import: "default", eager: true }) as Record<
  string,
  string
>;

/** Parse a scene file; throws with the file name and the problem if it's malformed. */
export function parseScene(id: string, text: string): TourScene {
  const match = text.match(/^---\n([\s\S]*?)\n---\n?([\s\S]*)$/);
  if (!match) throw new Error(`tour scene ${id}: expected a settings block between --- lines at the top`);
  const settings = new Map<string, string>();
  for (const line of match[1]!.split("\n")) {
    if (!line.trim() || line.trim().startsWith("#")) continue;
    const colon = line.indexOf(":");
    if (colon < 0) throw new Error(`tour scene ${id}: expected "key: value", got ${JSON.stringify(line)}`);
    // A value may be quoted, e.g. to contain a colon.
    settings.set(
      line.slice(0, colon).trim(),
      line
        .slice(colon + 1)
        .trim()
        .replace(/^"(.*)"$/, "$1"),
    );
  }
  const need = (key: string) => {
    const v = settings.get(key);
    if (!v) throw new Error(`tour scene ${id}: missing ${key}`);
    return v;
  };
  const num = (key: string, fallback: number) => {
    const v = settings.get(key);
    if (v === undefined) return fallback;
    const n = Number(v);
    if (!Number.isFinite(n)) throw new Error(`tour scene ${id}: ${key} must be a number, got ${JSON.stringify(v)}`);
    return n;
  };
  const ctcfText = settings.get("ctcfs") ?? "none";
  const ctcfs =
    ctcfText === "none" || ctcfText === ""
      ? []
      : ctcfText.split(",").map((part) => {
          const m = part.trim().match(/^(\d+)\s*([<>])$/);
          if (!m)
            throw new Error(
              `tour scene ${id}: a CTCF is a site and > or <, like "750 >"; got ${JSON.stringify(part.trim())}`,
            );
          return { site: Number(m[1]), left: m[2] === "<" };
        });
  return {
    id,
    title: need("title"),
    view: need("view"),
    preset: {
      nSites: num("lattice", DEFAULT_N_SITES),
      nLefs: num("lefs", 20),
      ctcfs,
      strength: num("strength", DEFAULT_STRENGTH),
      rates: {
        step: num("step", DEFAULT_RATES.step),
        respawn: num("unload", DEFAULT_RATES.respawn),
        respawnBlocked: num("unload_blocked", DEFAULT_RATES.respawnBlocked),
        respawnCaptured: num("unload_held", DEFAULT_RATES.respawnCaptured),
      },
      seed: num("seed", 1),
      speed: num("speed", 100),
    },
    html: marked.parse(match[2]!.trim(), { async: false }),
  };
}

/** The tour's scenes, in file-name order. */
export const SCENES: TourScene[] = Object.entries(files)
  .sort(([a], [b]) => a.localeCompare(b))
  .map(([path, text]) => parseScene(path.replace(/^.*\/|\.md$/g, ""), text));
