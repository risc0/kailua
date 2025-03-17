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

RUN --mount=type=cache,target=/root/.cargo/registry \
    --mount=type=cache,target=/kailua/target \
    cargo install -F prove --locked --path bin/cli --jobs 1

ENTRYPOINT ["kailua-cli"]