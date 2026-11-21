default:
    @just --list

clean: clean-cxx clean-rs

test: test-rs test-cxx

[positional-arguments]
check-rs *ARGS:
    cargo +stable check --locked "$@"
    just check-rs-formatted
    just check-rs-clippy

check-rs-formatted:
    cargo +stable fmt -- --check

check-rs-clippy:
    cargo +stable clippy --locked --all-targets --all-features -- -D warnings

[positional-arguments]
test-rs *ARGS:
    cargo +stable test --locked "$@"

[positional-arguments]
doc-rs *ARGS:
    cargo +stable doc --locked "$@"

[working-directory('ports/varpage-cxx')]
setup-cmake-build:
    cmake -S . -B build

[working-directory('ports/varpage-cxx')]
compile-cxx: setup-cmake-build
    cmake --build build

[working-directory('ports/varpage-cxx')]
test-cxx: compile-cxx
    ctest --test-dir build --output-on-failure --no-tests=error

[working-directory('ports/varpage-cxx')]
clean-cxx:
    rm -rf build

clean-rs:
    cargo clean
