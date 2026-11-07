# Rust harness: blazehttp ResponseParser (path dependency on crates/engine-core), alone and with the per-field split,
# and httparse.
# Build context: the repository root (Dockerfile.dockerignore keeps it to the files copied below).
# rust:1-bookworm resolved to rustc 1.99.1 (the repo toolchain, rust-toolchain.toml) at this digest.
FROM rust:1-bookworm@sha256:e6f2cff35d0f309acf8433b4f05878ed77092602c5065cc81801177da0bb3a9c

# Same CPU baseline as every other harness (design §16.2): generic AArch64 (Armv8-A).
ENV RUSTFLAGS="-C target-cpu=generic"

WORKDIR /src
COPY Cargo.toml ./
COPY crates/engine-core/Cargo.toml crates/engine-core/Cargo.toml
COPY crates/engine-core/src crates/engine-core/src
COPY bench/parsers/rust bench/parsers/rust
RUN cargo build --release --locked --manifest-path bench/parsers/rust/Cargo.toml \
 && cp bench/parsers/rust/target/release/parsers-bench-rust /usr/local/bin/harness \
 && { echo "blazehttp working tree (the runner records the commit)"; \
      echo "httparse $(cargo pkgid --offline --manifest-path bench/parsers/rust/Cargo.toml httparse)"; \
      echo "memchr (field split) $(cargo pkgid --offline --manifest-path bench/parsers/rust/Cargo.toml memchr)"; \
      rustc --version; echo "flags release lto=fat codegen-units=1 ${RUSTFLAGS}"; } > /versions.txt

ENTRYPOINT ["harness"]
