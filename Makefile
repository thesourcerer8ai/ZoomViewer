all:
	cargo run -r

test:
	cargo test

alltests:
	cargo test && cargo test -- --ignored

# Measure test coverage using cargo-llvm-cov.
# On first run this installs the required llvm-tools rustup component and the
# cargo-llvm-cov binary automatically.  Subsequent runs are fast.
#
# Output:
#   • terminal summary (per-file line/region/branch %)
#   • HTML report  →  target/llvm-cov/html/index.html
coverage:
	rustup component add llvm-tools-preview --toolchain stable
	cargo install cargo-llvm-cov --locked
	cargo llvm-cov --all-features --html
	@echo ""
	@echo "HTML report: target/llvm-cov/html/index.html"

# Same as 'coverage' but also runs the ignored (slow/integration) tests.
coverage-all:
	rustup component add llvm-tools-preview --toolchain stable
	cargo install cargo-llvm-cov --locked
	cargo llvm-cov --all-features --include-ignored --html
	@echo ""
	@echo "HTML report: target/llvm-cov/html/index.html"
