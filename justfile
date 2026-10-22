# Default recipes for Rust projects (seeded by `dev init rust`).

default:
    @just --list

build:
    cargo build

release:
    cargo build --release

run *args:
    cargo run -- {{args}}

test:
    cargo nextest run
    cargo test --doc

watch:
    bacon clippy-all

fmt:
    cargo fmt

lint:
    cargo fmt --check
    cargo clippy --all-targets -- -D warnings

clean:
    cargo clean
