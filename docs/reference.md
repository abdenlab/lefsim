# API reference

Each language's API reference comes from its own documentation tool. Until
they're published, build them locally:

| | Reference | Build it with |
|---|---|---|
| Rust | rustdoc, from the doc comments | `cargo doc --open -p lefsim` |
| Python | the type stubs and docstrings in `python/lefsim/_lefsim.pyi` | `help(lefsim.Sim)` in Python, or your editor |
| TypeScript | TSDoc comments in `packages/lefsim/src` | your editor (a TypeDoc build is to come) |

Next steps for this page: render the Python reference from the stubs with
mkdocstrings (which Zensical supports), generate the TypeScript reference with
TypeDoc, and link the rustdoc build.
