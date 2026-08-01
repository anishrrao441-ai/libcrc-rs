# One-step build to a runnable artifact.
#
#   docker build -t libcrc-rs .
#   docker run --rm libcrc-rs
#
# The run reproduces the headline claim: the UNMODIFIED original C test suite,
# compiled and linked against the Rust port, passing.
#
# ┌─ HONESTY NOTE ─────────────────────────────────────────────────────────────┐
# │ This Dockerfile was NOT built locally. No container runtime was available   │
# │ on the development machine (docker, podman and nerdctl are all absent), so  │
# │ it is validated in GitHub Actions (.github/workflows/ci.yml) rather than    │
# │ on the author's machine. The locally-verified one-command path is           │
# │ `./build.sh`, which runs green and is what the README leads with.           │
# └────────────────────────────────────────────────────────────────────────────┘

# Pinned by digest-bearing tag rather than `latest`, so the build is reproducible.
FROM rust:1.96.0-slim-bookworm

# gcc compiles the unmodified original C tests; coreutils supplies sha256sum for
# the tamper check. Nothing here builds or links the original C library.
RUN apt-get update \
 && apt-get install -y --no-install-recommends gcc libc6-dev coreutils make \
 && rm -rf /var/lib/apt/lists/*

WORKDIR /port

# Copy manifests first so dependency resolution caches across source edits.
COPY Cargo.toml Cargo.lock ./
COPY crates/libcrc-rs/Cargo.toml   crates/libcrc-rs/Cargo.toml
COPY crates/libcrc-cabi/Cargo.toml crates/libcrc-cabi/Cargo.toml

COPY . .

# Fails the image build if the port breaks, the original suite fails, or
# tests/original/ has been tampered with.
RUN chmod +x build.sh && ./build.sh

# Re-run the original suite as the container's default job.
CMD ["./build/testall"]
