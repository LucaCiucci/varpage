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

# Verify the crate package without publishing.
test-publish-rs:
    cargo +stable publish --dry-run --allow-dirty --locked

# Require a clean checkout tagged v<crate version>.
check-on-release-tag:
    #!/bin/sh
    set -eu
    status=$(git status --porcelain --untracked-files=normal)
    if [ -n "$status" ]; then
        echo "Commit or stash working tree changes before publishing." >&2
        exit 1
    fi

    package_id=$(cargo +stable pkgid --locked)
    version=${package_id##*[#@]}
    tag="v$version"
    head=$(git rev-parse --verify HEAD)
    tagged_commit=$(git rev-parse --verify --quiet "refs/tags/$tag^{commit}" || true)
    if [ -z "$tagged_commit" ]; then
        printf 'Missing release tag %s. To tag this commit, run: git tag -a %s -m "%s"\n' \
            "$tag" "$tag" "$tag" >&2
        exit 1
    fi
    if [ "$tagged_commit" != "$head" ]; then
        printf 'Release tag %s points to a different commit than HEAD.\n' "$tag" >&2
        exit 1
    fi

# Publish the tagged Rust crate to crates.io.
publish-rs: check-on-release-tag test-publish-rs
    cargo +stable publish --locked
