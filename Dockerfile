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

# Install the RISC0 toolchain
# RUN curl -L https://risczero.com/install | bash && \
#     . $HOME/.bashrc && \
#     export PATH="$HOME/.rzup/bin:$PATH" && \
#     rzup install r0vm 1.2.5 -v

# Install Solidity compiler
RUN curl -L https://github.com/ethereum/solidity/releases/download/v0.8.26/solc-static-linux -o /usr/local/bin/solc && \
    chmod +x /usr/local/bin/solc

# RUN curl -L https://foundry.paradigm.xyz | bash && \
#     . $HOME/.bashrc && \
#     chmod +x $HOME/.foundry/bin/foundryup && \
#     foundryup

# Now copy the actual source code
COPY . .

RUN cargo install svm-rs && \
    svm install 0.8.24
# RUN cargo build -p kailua-contracts

# Build dependencies first, then install the CLI
RUN --mount=type=cache,target=/root/.cargo/registry \
    --mount=type=cache,target=/kailua/target \
    cargo install -F prove --locked --path bin/cli --jobs $(nproc)

ENTRYPOINT ["kailua-cli"]