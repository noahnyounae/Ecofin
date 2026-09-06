# syntax=docker/dockerfile:1.7
#
# Build en quatre temps, agencé pour ne jamais recompiler ce qui n'a pas changé.
#
#   web-builder → compile le front React
#   planner     → calcule la liste des dépendances Rust (cargo-chef)
#   builder     → compile les dépendances à part, puis les binaires
#   runtime     → image finale, sans chaîne de compilation
#
# La couche de dépendances Rust n'est invalidée que par Cargo.toml ou
# Cargo.lock ; celle du front, que par package.json. Modifier du code source ne
# recompile donc que ce code.
#
# Pour une boucle de développement plus courte encore, voir le service `dev` de
# docker-compose.yml : il garde target/ dans un volume entre deux exécutions.

ARG RUST_VERSION=1.93
ARG NODE_VERSION=22

# Nombre de compilations Rust menées de front.
#
# Par défaut cargo en lance autant que de cœurs, chacune pouvant réclamer près
# d'un gigaoctet. Avec les 3,8 Go alloués par défaut à Docker Desktop, huit
# compilations simultanées épuisent la mémoire et font tomber le daemon en
# plein build — constaté ici.
#
# Deux passent sans risque à 3,8 Go. Après avoir relevé la mémoire de Docker
# (Settings → Resources), `--build-arg CARGO_JOBS=8` rend le build bien plus
# rapide.
ARG CARGO_JOBS=2

# ---------------------------------------------------------------------------

FROM node:${NODE_VERSION}-bookworm-slim AS web-builder

WORKDIR /web

# Les dépendances d'abord : leur couche survit à toute modification du code.
COPY web/package.json web/package-lock.json* ./
RUN npm ci --no-audit --no-fund

COPY web/ ./
RUN npm run build

# ---------------------------------------------------------------------------

FROM rust:${RUST_VERSION}-bookworm AS chef

# cmake est exigé par aws-lc-rs (la brique cryptographique de rustls et des
# JWT) ; rusqlite en mode `bundled` compile SQLite, d'où le compilateur C.
RUN apt-get update \
 && apt-get install -y --no-install-recommends cmake \
 && rm -rf /var/lib/apt/lists/*

RUN cargo install cargo-chef --locked --version ^0.1

WORKDIR /app

# ---------------------------------------------------------------------------

FROM chef AS planner

COPY Cargo.toml Cargo.lock ./
COPY crates ./crates

# recipe.json ne décrit que les dépendances, pas le code : il ne change pas
# quand crates/ change, ce qui rend la couche suivante réutilisable.
RUN cargo chef prepare --recipe-path recipe.json

# ---------------------------------------------------------------------------

FROM chef AS builder

ARG CARGO_JOBS
ENV CARGO_BUILD_JOBS=${CARGO_JOBS}

COPY --from=planner /app/recipe.json recipe.json

# L'étape longue, et la seule qu'on cherche à éviter de rejouer. Le résultat
# est écrit dans target/ et conservé dans la couche : pas de montage de cache
# ici, sinon les artefacts ne survivraient pas à l'étape suivante.
RUN cargo chef cook --release --recipe-path recipe.json

COPY Cargo.toml Cargo.lock ./
COPY crates ./crates

RUN cargo build --release --locked --workspace \
 && strip target/release/ecofin target/release/ecofin-web

# ---------------------------------------------------------------------------

FROM debian:bookworm-slim AS runtime

# ca-certificates : sans lui, la vérification TLS de api.enablebanking.com
# échoue. poppler-utils fournit pdftotext, dont `ecofin import` a besoin pour
# lire les relevés PDF. SQLite, lui, est compilé dans les binaires.
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates poppler-utils \
 && rm -rf /var/lib/apt/lists/*

# Le conteneur manipule des données bancaires : pas de root.
RUN useradd --create-home --uid 10001 ecofin

COPY --from=builder /app/target/release/ecofin /usr/local/bin/ecofin
COPY --from=builder /app/target/release/ecofin-web /usr/local/bin/ecofin-web
COPY --from=web-builder /web/dist /srv/web

# Configuration, clé privée et base vivent ici — à monter en volume, sans quoi
# la connexion bancaire serait à refaire à chaque démarrage.
ENV ECOFIN_HOME=/data
ENV ECOFIN_WEB_ROOT=/srv/web
RUN mkdir -p /data && chown ecofin:ecofin /data
VOLUME ["/data"]

USER ecofin
WORKDIR /data

# 8484 : retour d'autorisation bancaire, le temps d'un `link new`.
# 8080 : application web.
EXPOSE 8484 8080

ENTRYPOINT ["ecofin"]
CMD ["--help"]
