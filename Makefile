# Entry points only; the build logic lives in xtask/src.
.DEFAULT_GOAL := help
.PHONY: help build run check-dictation selftest esctest ghostty-vt icon lint check perf fuzz fuzz-parser release release-notarized deploy

help: ## List the targets
	@awk 'BEGIN {FS = ":.*## "} /^[a-z-]+:.*## / {printf "  %-12s %s\n", $$1, $$2}' $(MAKEFILE_LIST)

build: ## Build the release app with dictation (whisper.cpp needs cmake) into target/release, signed ad hoc for this Mac
	cargo xtask bundle --release

run: ## Build the debug app into target/debug and open it
	cargo xtask run

selftest: ## Run STEPS (comma-separated actions, type:, wait:, shot:) in the app; pictures in target/selftest
	cargo xtask selftest "$(STEPS)"

esctest: ## esctest in the app (or INCLUDE=<regex>): counts and failing tests
	cargo xtask esctest $(if $(INCLUDE),--include '$(INCLUDE)')

ghostty-vt: ## Build libghostty-vt (needs zig and network) for term-fuzz's comparison with Ghostty, which is skipped without it
	cargo xtask ghostty-vt

icon: ## Render assets/logo/app-icon.svg into resources/AppIcon.icns (needs resvg)
	cargo xtask icon

lint: ## Formatting, clippy with warnings as errors, the files' shape and the interface texts
	cargo fmt --all --check
	cargo clippy --workspace --all-targets -- -D warnings
	cargo clippy --workspace --all-targets --features tapa/selftest,term-session/fuzzing -- -D warnings
	cargo xtask structure
	cargo xtask strings

check-dictation: ## Clippy and the tests of the app and term-ui-dictation with dictation, in release as the bundle builds it (whisper.cpp with cmake)
	cargo clippy --release -p tapa -p term-ui-dictation --all-targets --features tapa/dictation,term-ui-dictation/engine -- -D warnings
	cargo clippy --release -p tapa -p term-ui-dictation --all-targets --features tapa/dictation,tapa/selftest,term-ui-dictation/engine,term-ui-dictation/selftest -- -D warnings
	cargo test --release -p term-ui-dictation --features engine

check: lint ## lint, then all tests; needs no window and no network
	cargo test --workspace

perf: ## The bench of the plan's section 3 (E6) against the pinned Ghostty: spread, crate budgets, start and vtebench throughput of both; meaningful only on an idle Mac
	cargo xtask perf spread
	cargo xtask perf crates
	cargo xtask bundle --release
	cargo xtask perf start
	cargo xtask perf start ghostty
	cargo xtask perf throughput
	cargo xtask perf throughput ghostty

release: ## Dry run of the release with dictation: universal app, disk image, update feed and Cask in target/release-dry-run; sends nothing anywhere
	cargo xtask release

release-notarized: check ## The current version: Developer ID, notarized by Apple, into dist/; the version unchanged
	cargo xtask release --notarize

deploy: ## Notarize the next version, commit it in Cargo.toml, upload the draft, push main and its tag, then publish the release; ARGS=--minor raises the minor version, ARGS=--major the major version
	cargo xtask deploy $(ARGS)

fuzz: ## The fuzz targets with cargo-fuzz and the nightly it needs, FUZZ_SECONDS each (60 by default), at most 2 GB each
	cd fuzz && for target in scan procargs terminal; do PATH="$$(dirname $$(rustup which --toolchain nightly cargo)):$$PATH" CARGO_BUILD_JOBS=2 cargo fuzz run --fuzz-dir term-session $$target -- -max_total_time=$${FUZZ_SECONDS:-60} -rss_limit_mb=2048; done

fuzz-parser: ## The parser and screen fuzzed for FUZZ_SECONDS (600 by default) with at most 2 GB, for CI to call (F183)
	cd fuzz && PATH="$$(dirname $$(rustup which --toolchain nightly cargo)):$$PATH" CARGO_BUILD_JOBS=2 cargo fuzz run --fuzz-dir term-session terminal -- -max_total_time=$${FUZZ_SECONDS:-600} -rss_limit_mb=2048 -timeout=10
