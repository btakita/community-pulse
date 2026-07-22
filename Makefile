.PHONY: check fmt clippy test build demo demo-mobile demo-companion demo-shots demo-launcher-smoke

check: fmt clippy test demo-launcher-smoke

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

demo-launcher-smoke: build
	PULSE_BINARY=./target/debug/pulse ./bin/desktop-and-mobile.sh --help >/dev/null
