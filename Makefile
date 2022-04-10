.PHONY: test format

test:
	cargo fmt -- --check
	cargo-sort --check --workspace
	cargo clippy --all-features --workspace
	cargo test --all-features --workspace

format:
	cargo fmt
	cargo-sort --workspace
