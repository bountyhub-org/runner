FROM --platform=$BUILDPLATFORM rust:1.89.0 AS base

RUN cargo install sccache --version ^0.10
RUN cargo install cargo-chef --version ^0.1
ENV RUSTC_WRAPPER=sccache SCCACHE_DIR=/sccache

FROM --platform=$BUILDPLATFORM base AS planner
WORKDIR /home/runner
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=$SCCACHE_DIR,sharing=locked \
    cargo chef prepare --recipe-path recipe.json

FROM base AS builder

WORKDIR /home/runner

COPY --from=planner /home/runner/recipe.json recipe.json
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=$SCCACHE_DIR,sharing=locked \
    cargo chef cook --release --recipe-path recipe.json

COPY . .

RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=$SCCACHE_DIR,sharing=locked \
    SKIP_TWIRP_GENERATION=1 cargo build --release -p cli

FROM --platform=$BUILDPLATFORM rust:1.87.0 AS tools

RUN cargo install bh

FROM scratch

WORKDIR /home/runner

COPY --from=builder /home/runner/target/release/cli /runner
COPY --from=tools /usr/local/cargo/bin/bh /bh
