# TODO alpine is smaller
FROM rust:1.81 as build-environment

ARG CARGO_BUILD_JOBS=1

RUN apt-get update -y && apt-get install -y --no-install-recommends \
    build-essential \
    pkg-config \
    libclang-dev \
    clang \
    curl \
    && rm -rf /var/lib/apt/lists/*

RUN cargo install svm-rs && \
    svm install 0.8.24

WORKDIR /kailua

COPY . .

RUN --mount=type=cache,target=/root/.cargo/registry,sharing=shared \
    --mount=type=cache,target=/root/.cargo/git,sharing=shared \
    --mount=type=cache,target=/kailua/target,sharing=private,id=rust-target-${TARGETARCH} \
    cargo build --jobs ${CARGO_BUILD_JOBS} --release -F disable-dev-mode \
    && mkdir out \
    && mv target/release/kailua-cli out/ \
    && strip out/kailua-cli;

FROM rust:1.81 as kailua
COPY --from=build-environment /kailua/out/kailua-cli /usr/local/bin/kailua-cli

ENTRYPOINT ["/bin/sh", "-c"]
