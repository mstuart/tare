.PHONY: build test fmt lint doc deny check proxy bench clean

build:
	cargo build --workspace --release

test:
	cargo test --workspace

fmt:
	cargo fmt --all

lint:
	cargo clippy --workspace --all-targets -- -D warnings

doc:
	cargo doc --no-deps --workspace

deny:
	cargo deny check

proxy:
	cargo run --release -p tare-proxy

bench:
	cargo run --release -p tare-bench

# The full CI gate, locally.
check:
	cargo fmt --all --check
	cargo clippy --workspace --all-targets -- -D warnings
	cargo test --workspace
	cargo build --workspace --release

clean:
	cargo clean
