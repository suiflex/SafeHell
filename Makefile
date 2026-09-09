.PHONY: all build release run serve tray test lint fmt check clean

all: build

build:
	cargo build

release:
	cargo build --release

run:
	cargo run

serve:
	cargo run -- serve

tray:
	cargo run -- tray

test:
	cargo test --all-targets

lint:
	cargo clippy --all-targets -- -D warnings

fmt:
	cargo fmt --all

fmt-check:
	cargo fmt --all -- --check

test-scripts:
	sh tests/install_sh_test.sh
	node npm/install.js --selftest
	sh tests/logo.sh --check
	python3 .github/scripts/test_attribute_changelog.py
check: fmt-check lint test test-scripts

clean:
	cargo clean
