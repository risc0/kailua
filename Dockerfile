# TODO alpine is smaller
FROM --platform=arm64 rust:1.81

# Install build dependencies with fixed GPG keys
RUN apt-get update -y && apt-get install -y --no-install-recommends gnupg && \
    apt-key update && \
    apt-get update -y && apt-get install -y \
    build-essential \
    pkg-config \
    libclang-dev \
    clang \
    curl \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /kailua

# Set environment variables for faster builds
ENV CARGO_BUILD_JOBS=4
ENV CARGO_NET_RETRY=5

COPY . .

RUN cargo install svm-rs && \
    svm install 0.8.24

RUN --mount=type=cache,target=/root/.cargo/registry \
    --mount=type=cache,target=/kailua/target \
    cargo install -F prove --locked --path bin/cli --jobs $(nproc)

ENTRYPOINT ["kailua-cli"]