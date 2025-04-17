FROM ghcr.io/foundry-rs/foundry:v1.0.0

# Switch to root, install jq, then switch back to foundry user all in one layer
USER root
RUN apt-get update && apt-get install -y jq && apt-get clean && rm -rf /var/lib/apt/lists/*
USER foundry

WORKDIR /app
COPY . .

RUN git init
RUN forge install
RUN forge build
