.PHONY: dev run build test fmt fmt-check lint clippy migrate sqlx-prepare \
        compose-up compose-down docker-build deploy clean help

# ----------------------------------------------------------------------------
# Defaults
# ----------------------------------------------------------------------------
DATABASE_URL ?= postgres://mizan:mizan@localhost:5433/mizan_connect
DOCKER_IMAGE ?= mizan-connect
DOCKER_TAG   ?= latest

export DATABASE_URL

help:
	@echo "Targets:"
	@echo "  dev            Start docker compose + run server (foreground)"
	@echo "  run            Run server (assumes Postgres already up)"
	@echo "  build          cargo build --release"
	@echo "  test           cargo test --workspace"
	@echo "  fmt            cargo fmt --all"
	@echo "  fmt-check      cargo fmt --all -- --check"
	@echo "  lint           fmt-check + clippy with -D warnings"
	@echo "  clippy         cargo clippy --all-targets --all-features -- -D warnings"
	@echo "  migrate        sqlx migrate run"
	@echo "  sqlx-prepare   cargo sqlx prepare (refresh sqlx-data.json)"
	@echo "  compose-up     docker compose up -d"
	@echo "  compose-down   docker compose down"
	@echo "  docker-build   build production Dockerfile"
	@echo "  deploy         fly deploy"
	@echo "  clean          cargo clean + remove .sqlx caches"

dev: compose-up migrate run

run:
	cargo run

build:
	cargo build --release

test:
	cargo test --workspace -- --nocapture

fmt:
	cargo fmt --all

fmt-check:
	cargo fmt --all -- --check

clippy:
	cargo clippy --all-targets --all-features -- -D warnings

lint: fmt-check clippy

migrate:
	sqlx migrate run

sqlx-prepare:
	cargo sqlx prepare -- --tests

compose-up:
	docker compose up -d

compose-down:
	docker compose down

docker-build:
	docker build -t $(DOCKER_IMAGE):$(DOCKER_TAG) .

deploy:
	fly deploy

clean:
	cargo clean
	rm -rf .sqlx
