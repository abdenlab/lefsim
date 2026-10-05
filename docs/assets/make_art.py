"""Draw the docs' banner and logo.

The banner is a real snapshot: loops as arcs over the lattice, in the browser
demos' style and colors (blue extruding, orange held at a CTCF). Run from
python/, where lefsim is installed:

    uv run python ../docs/assets/make_art.py
"""

from pathlib import Path

import lefsim
import polars as pl

OUT = Path(__file__).parent

THEMES = {
    "light": {"ink": "#55585f", "extruding": "#2a78d6", "held": "#eb6834"},
    "dark": {"ink": "#b9bcc2", "extruding": "#3987e5", "held": "#d95926"},
}

# Three domains, each between a convergent pair of CTCFs.
N_SITES = 600
CTCFS = [
    (60, "left"), (200, "right"),
    (230, "left"), (390, "right"),
    (420, "left"), (540, "right"),
]
W, H = 1200, 260
MARGIN_X, BASE = 24, 220
MAX_H = BASE - 16


def snapshot():
    """A steady-state snapshot with plenty of loops held at boundaries."""
    lattice = lefsim.Lattice(
        N_SITES,
        {"site": [s for s, _ in CTCFS], "captures_from": [c for _, c in CTCFS], "strength": 0.9},
    )
    rates = lefsim.Rates(step=1.0, respawn=2e-3, respawn_blocked=1e-2, respawn_captured=1e-3)
    sim = lefsim.Sim(lefsim.Model(lefsim.populate(lattice, 14, seed=3), rates), seed=11)
    rec = sim.record_until(20_000.0, dt=50.0)
    snaps = pl.DataFrame(rec.snapshots).filter(pl.col("time") >= 5_000.0)
    held = (pl.col("left_mobility") == "Captured") | (pl.col("right_mobility") == "Captured")
    snaps = snaps.with_columns(held=held)
    # The grid point with the most held loops, ties broken by total loop size.
    best = (
        snaps.group_by("time")
        .agg(pl.col("held").sum(), (pl.col("right_pos") - pl.col("left_pos")).sum().alias("size"))
        .sort(["held", "size"], descending=True)
        .item(0, "time")
    )
    return snaps.filter(pl.col("time") == best).sort("held")


def x(site):
    return MARGIN_X + (W - 2 * MARGIN_X) * site / (N_SITES - 1)


def banner(lefs, theme):
    c = THEMES[theme]
    full = max(b - a for (a, _), (b, _) in zip(CTCFS[::2], CTCFS[1::2]))
    out = [
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {W} {H}" role="img" '
        'aria-label="Loops extruded by LEFs on a lattice, drawn as arcs, some held at CTCF boundaries">',
        f'<line x1="{x(0):.1f}" y1="{BASE}" x2="{x(N_SITES - 1):.1f}" y2="{BASE}" '
        f'stroke="{c["ink"]}" stroke-width="1.5"/>',
    ]
    for site, side in CTCFS:
        d = -1 if side == "left" else 1
        cx, top, size = x(site) + 2 * d, BASE + 8, 14
        out.append(
            f'<path d="M{cx + d * size * 0.6:.1f} {top + size / 2} L{cx:.1f} {top} L{cx:.1f} {top + size} Z" '
            f'fill="{c["ink"]}"/>'
        )
    for lef in lefs.iter_rows(named=True):
        color = c["held"] if lef["held"] else c["extruding"]
        x1, x2 = x(lef["left_pos"]), x(lef["right_pos"])
        span = lef["right_pos"] - lef["left_pos"]
        h = min(MAX_H, max(3, MAX_H * span / full))
        r = max((x2 - x1) / 2, 0.5)
        out.append(
            f'<path d="M{x1:.1f} {BASE} A{r:.1f} {h:.1f} 0 0 1 {x2:.1f} {BASE}" '
            f'fill="none" stroke="{color}" stroke-width="3" stroke-linecap="round"/>'
        )
        for xp in (x1, x2):
            out.append(f'<rect x="{xp - 2:.1f}" y="{BASE - 7}" width="4" height="9" fill="{color}"/>')
    out.append("</svg>")
    return "\n".join(out) + "\n"


def logo(outer, inner, base):
    """Two nested loops on a baseline, on a 24-unit grid."""
    return (
        '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" '
        'stroke-width="2" stroke-linecap="round">\n'
        f'<path d="M2 19 H22" stroke="{base}"/>\n'
        f'<path d="M4 19 A8 13 0 0 1 20 19" stroke="{outer}"/>\n'
        f'<path d="M8.5 19 A3.5 7 0 0 1 15.5 19" stroke="{inner}"/>\n'
        "</svg>\n"
    )


if __name__ == "__main__":
    lefs = snapshot()
    for theme in THEMES:
        (OUT / f"banner-{theme}.svg").write_text(banner(lefs, theme))
    # Mid-tone colors, legible on light and dark headers and browser tabs.
    (OUT / "logo.svg").write_text(logo("#eb6834", "#2a78d6", "#8a8d93"))
    print(f"t = {lefs['time'][0]}, {lefs['held'].sum()} of {len(lefs)} loops held")
