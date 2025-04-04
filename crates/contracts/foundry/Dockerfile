FROM ghcr.io/foundry-rs/foundry:stable

WORKDIR /app
COPY . .

RUN git init

RUN forge install

RUN forge build
