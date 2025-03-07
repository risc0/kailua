FROM --platform=linux/amd64 rust:1.82.0-bullseye

# Install build dependencies
RUN apt-get update && apt-get install -y \
    build-essential \
    pkg-config \
    libclang-dev \
    clang \
    curl \
    && rm -rf /var/lib/apt/lists/*

# Create and set working directory
WORKDIR /kailua

# Copy all files (except those in .gitignore)
COPY . .

# Install RISC0 toolchain
RUN curl -L https://risczero.com/install | bash && \
    . $HOME/.bashrc && \
    export PATH="$HOME/.rzup/bin:$PATH" && \
    rzup install

# Build the CLI
RUN cargo install kailua-cli --path bin/cli --locked --debug

# Set the entrypoint to the CLI
ENTRYPOINT ["kailua-cli"] 