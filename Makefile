.PHONY: check fmt clippy test demo

check: fmt clippy test

fmt:
	cargo fmt --all -- --check

clippy:
	cargo clippy --all-targets --all-features -- -D warnings

test:
	cargo test --all-features

demo:
	cargo run -- --fixture --replay app
