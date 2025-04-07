# TODO alpine is smaller
FROM rust:1.81

RUN apt-get update -y && apt-get install -y --no-install-recommends \
    build-essential \
    pkg-config \
    libclang-dev \
    clang \
    curl \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /kailua

COPY . .

RUN cargo install svm-rs && \
    svm install 0.8.24

RUN --mount=type=cache,target=/root/.cargo/registry --mount=type=cache,target=/root/.cargo/git --mount=type=cache,target=/kailua/target \
    cargo build --jobs 4 \
    && mkdir out \
    && mv target/debug/kailua-host out/ \
    && mv target/debug/kailua-cli out/ \
    && mv target/debug/kailua-client out/ \
    && strip out/kailua-host \
    && strip out/kailua-cli \
    && strip out/kailua-client;

ENTRYPOINT ["/bin/sh", "-c"]
