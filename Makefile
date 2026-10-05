# Shortcuts for the commands in AGENTS.md and the README. `make` lists them.
# Written for GNU Make 3.81 (the macOS default) and later.

MDFORMAT := uvx --with mdformat-mkdocs --with mdformat-tables mdformat
MARKDOWN := docs/ README.md AGENTS.md CHANGELOG.md crates/lefsim/README.md python/README.md packages/lefsim/README.md

.DEFAULT_GOAL := help

.PHONY: help
help: ## List the targets
	@grep -E '^[a-z][a-z-]*:.*## ' $(MAKEFILE_LIST) | awk -F':.*## ' '{printf "  %-14s %s\n", $$1, $$2}'

# ---------------------------------------------------------------- setup

.PHONY: setup
setup: ## Create the Python venv (with the extension) and install the JS dependencies
	cd python && uv sync
	pnpm install

# ---------------------------------------------------------------- build

.PHONY: wasm
wasm: ## Build the wasm module into packages/lefsim/wasm
	pnpm build:wasm

.PHONY: py
py: ## Sync the venv, rebuilding the extension if the Rust code changed
	cd python && uv sync

# ---------------------------------------------------------------- test

.PHONY: test
test: test-rust test-python test-ts ## Run all three test suites

.PHONY: test-rust
test-rust: ## Rust tests (core crate, all features)
	cargo test -p lefsim --all-features

.PHONY: test-python
test-python: ## Python tests (uv rebuilds the extension if the Rust code changed)
	cd python && uv run pytest tests

.PHONY: test-ts
test-ts: wasm ## TypeScript tests and typecheck, after rebuilding the wasm
	pnpm test
	pnpm typecheck

# ---------------------------------------------------------------- lint and format

.PHONY: lint
lint: ## Check formatting and clippy, as CI does
	cargo fmt --all --check
	cargo clippy --workspace --all-targets --all-features -- -D warnings
	$(MDFORMAT) --check $(MARKDOWN)
	scripts/check-version.sh

.PHONY: fmt
fmt: ## Format the Rust code and the Markdown
	cargo fmt --all
	$(MDFORMAT) $(MARKDOWN)

# ---------------------------------------------------------------- docs and demos

.PHONY: docs
docs: ## Build the docs site into site/ (strict, as CI does)
	uvx zensical build --strict

.PHONY: serve
serve: ## Preview the docs with live reload
	uvx zensical serve

.PHONY: demo
demo: wasm ## Run the browser demos on a Vite dev server
	pnpm demo

# ---------------------------------------------------------------- release

.PHONY: bump
bump: ## Set the version everywhere: make bump VERSION=0.2.0
	@test -n "$(VERSION)" || (echo "usage: make bump VERSION=0.2.0" >&2; exit 1)
	scripts/bump-version.sh $(VERSION)

.PHONY: clean
clean: ## Remove build outputs (target/, wasm, dist/, site/)
	cargo clean
	rm -rf packages/lefsim/wasm packages/lefsim/dist packages/demo/dist site
