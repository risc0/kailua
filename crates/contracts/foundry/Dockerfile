FROM ghcr.io/foundry-rs/foundry@sha256:8b843eb65cc7b155303b316f65d27173c862b37719dc095ef3a2ef27ce8d3c00

RUN apk add --no-cache jq

WORKDIR /app
COPY . .

RUN git init

RUN forge install

RUN forge build
