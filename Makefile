.PHONY: verify fmt fmt-check check lint test doc-test run

verify: fmt-check check lint test doc-test

fmt:
	cargo fmt --all

fmt-check:
	cargo fmt --all --check

check:
	cargo check --workspace --all-targets --all-features --locked

lint:
	cargo clippy --workspace --all-targets --all-features --locked -- -D warnings

test:
	cargo test --workspace --all-targets --all-features --locked

doc-test:
	cargo test --doc --workspace --locked

run:
	cargo run --locked -- $(ARGS)
