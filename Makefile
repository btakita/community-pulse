.PHONY: check fmt clippy test demo demo-mobile demo-companion

check: fmt clippy test

fmt:
	cargo fmt --all -- --check

clippy:
	cargo clippy --all-targets --all-features -- -D warnings

test:
	cargo test --all-features

demo:
	cargo run -- --fixture --replay app

demo-mobile:
	cargo run -- --fixture --replay app --mobile

demo-companion:
	cargo run -- --fixture --replay app --companion
