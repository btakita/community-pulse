.PHONY: check fmt clippy test build demo demo-mobile demo-companion demo-shots

check: fmt clippy test

fmt:
	cargo fmt --all -- --check

clippy:
	cargo clippy --all-targets --all-features -- -D warnings

test:
	cargo test --all-features

build:
	cargo build --all-features

demo:
	cargo run -- --fixture --replay app

demo-mobile:
	cargo run -- --fixture --replay app --mobile

demo-companion:
	cargo run -- --fixture --replay app --companion

demo-shots: build
	xvfb-run -a -s "-screen 0 1920x1080x24" ./scripts/capture-demo-shots.sh
