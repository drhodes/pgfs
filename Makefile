PGFS := ./scripts/pgfs.sh

.PHONY: help up down status run build check clippy fmt init-db db-stop shell clean test test-integration ensure-db

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
	@echo "  make test      cargo test (Rust unit tests)"
	@echo "  make test-integration  run the full FUSE integration suite (scripts/run_tests.sh)"
	@echo "  make init-db   create/start the project-local Postgres cluster"
	@echo "  make db-stop   stop the project-local Postgres cluster"
	@echo "  make shell     nix develop"
	@echo "  make clean     cargo clean"

check-nix:
	@if [ -z "$$PGFS_ROOT" ]; then \
		echo "Error: Nix environment is not active."; \
		if ! nix-instantiate --eval --expr '1' >/dev/null 2>&1; then \
			if ! groups | grep -q '\bnix-users\b'; then \
				echo "Reason: Your user is not in the 'nix-users' group."; \
				echo "Fix: Run 'sudo usermod -aG nix-users $$USER' and log out/in (or run 'exec sg nix-users bash')."; \
			else \
				echo "Reason: Cannot access Nix daemon socket (/nix/var/nix/daemon-socket/socket)."; \
				echo "Fix: Ensure your active login shell session has the 'nix-users' group applied (e.g. run 'exec sg nix-users bash')."; \
			fi; \
		else \
			echo "Fix: Please enter the Nix dev shell using 'nix develop' or 'make shell'."; \
		fi; \
		exit 1; \
	fi

up: check-nix
	$(PGFS) up

down:
	$(PGFS) down

status:
	$(PGFS) status

run: check-nix
	$(PGFS) run

build: check-nix
	cargo build

check: check-nix
	cargo check

clippy: check-nix
	cargo clippy

fmt: check-nix
	cargo fmt

ensure-db: check-nix
	@./scripts/ensure_db.sh

test: check-nix ensure-db
	cargo test

test-integration: check-nix
	./scripts/run_tests.sh

init-db: check-nix
	./scripts/init_db.sh

db-stop:
	pg_ctl -D testdata/pgdata stop

shell:
	nix --extra-experimental-features 'nix-command flakes' develop

clean:
	cargo clean

packout:
	packout spec/*.py src/*.rs flake.nix scripts/* design/chats/* Cargo.toml build.rs > /tmp/out
	git log >> /tmp/out
	xclip -selection clipboard < /tmp/out
