"""Write a sample recording for the demo's Parquet page.

Runs the demo's model (a 3,000-site lattice split into five TADs by
convergent CTCF pairs) and writes a keyframed event log with
`Stream.to_parquet` to `public/sample.parquet`, which the page offers when
it's there. Run with the Python dev venv, from python/:

    uv run python ../packages/demo/make_sample.py [--lefs 30] [--t-end 100000]

Any file written this way can also be opened on the page, by picking it,
dropping it, or passing `?url=...`.
"""

import argparse
import pathlib

import lefsim

HERE = pathlib.Path(__file__).parent

parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
parser.add_argument("--lefs", type=int, default=30)
parser.add_argument("--t-end", type=float, default=100_000.0)
parser.add_argument("--seed", type=int, default=1)
parser.add_argument("--out", type=pathlib.Path, default=HERE / "public" / "sample.parquet")
args = parser.parse_args()

boundaries = [600, 1300, 1800, 2500]
lattice = lefsim.Lattice(
    3000,
    {
        "site": [s for b in boundaries for s in (b, b + 1)],
        "captures_from": ["left", "right"] * len(boundaries),
        "strength": 0.8,
    },
)
model = lefsim.Model(
    lefsim.populate(lattice, args.lefs, seed=args.seed),
    lefsim.Rates(step=1.0, respawn=0.002, respawn_blocked=0.02, respawn_captured=0.002),
)
args.out.parent.mkdir(parents=True, exist_ok=True)
lefsim.Sim(model, seed=args.seed).stream(args.t_end, events=True, keyframes=True, max_rows=20_000).to_parquet(args.out)
print(f"wrote {args.out} ({args.out.stat().st_size / 1e6:.1f} MB)")
