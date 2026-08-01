PGFS := ./scripts/pgfs.sh

.PHONY: help up down status run build check clippy fmt init-db db-stop shell clean

help:
	@echo "pgfs — PostgreSQL-backed FUSE filesystem"
	@echo
	@echo "Lifecycle (via scripts/pgfs.sh):"
	@echo "  make up        bring up Postgres + build + mount, wait until live"
	@echo "  make down      unmount cleanly"
	@echo "  make status    show mount / daemon / Postgres state"
	@echo "  make run       run pgfs in the foreground (Ctrl+C quits cleanly)"
	@echo
	@echo "Development:"
	@echo "  make build     cargo build"
	@echo "  make check     cargo check"
	@echo "  make clippy    cargo clippy"
	@echo "  make fmt       cargo fmt"
	@echo "  make init-db   create/start the project-local Postgres cluster"
	@echo "  make db-stop   stop the project-local Postgres cluster"
	@echo "  make shell     nix develop"
	@echo "  make clean     cargo clean"

up:
	$(PGFS) up

down:
	$(PGFS) down

status:
	$(PGFS) status

run:
	$(PGFS) run

build:
	cargo build

check:
	cargo check

clippy:
	cargo clippy

fmt:
	cargo fmt

init-db:
	./scripts/init_db.sh

db-stop:
	pg_ctl -D testdata/pgdata stop

shell:
	nix develop

clean:
	cargo clean
