# Changelog

All three packages (the `lefsim` crate, the Python wheel and the npm package)
share one version. While the version is 0.x, a minor release may break the API
in any language *or change seeded trajectories*; a patch release does neither.
Model JSON files carry their own format version.

## Unreleased

First release: the Rust core, Python bindings (Arrow output, Parquet event logs,
replay) and the WebAssembly/TypeScript package.
