.PHONY: dev dev-sfu dev-signaling test lint build docker

SFU_DIR := sfu
SIGNALING_DIR := signaling
REGIONCTL_DIR := regionctl
PROBER_DIR := prober
REVISION := $(shell git rev-parse --short HEAD 2>/dev/null || echo unknown)

dev:
	@echo "Run in two terminals:"
	@echo "  SFU_PUBLIC_IP=127.0.0.1 make dev-sfu"
	@echo "  set -a; . signaling/.env; set +a; make dev-signaling"

dev-sfu:
	cd $(SFU_DIR) && cargo run -p sfu

dev-signaling:
	cd $(SIGNALING_DIR) && mix deps.get && mix run --no-halt

test:
	cd $(SFU_DIR) && cargo test
	cd $(REGIONCTL_DIR) && cargo test
	cd $(PROBER_DIR) && cargo test --workspace --locked
	cd $(SIGNALING_DIR) && MIX_ENV=test mix deps.get && MIX_ENV=test mix test

lint:
	cd $(SFU_DIR) && cargo fmt --check && cargo clippy --all-targets -- -D warnings
	cd $(REGIONCTL_DIR) && cargo fmt --check && cargo clippy --all-targets -- -D warnings
	cd $(PROBER_DIR) && cargo fmt --all --check && cargo clippy --workspace --all-targets --locked -- -D warnings
	cd $(SIGNALING_DIR) && mix format --check-formatted && MIX_ENV=test mix credo --strict

build:
	cd $(SFU_DIR) && cargo build --release
	cd $(REGIONCTL_DIR) && cargo build --release
	cd $(PROBER_DIR) && cargo build --release --locked -p voice-prober-bin
	cd $(SIGNALING_DIR) && MIX_ENV=prod mix deps.get --only prod && MIX_ENV=prod mix release --overwrite

docker:
	docker build --build-arg REVISION=$(REVISION) -t ghcr.io/discordy-voics/voice-sfu:dev $(SFU_DIR)
	docker build --build-arg REVISION=$(REVISION) -t ghcr.io/discordy-voics/voice-signaling:dev $(SIGNALING_DIR)
	docker build --build-arg REVISION=$(REVISION) -t ghcr.io/discordy-voics/voice-prober:dev $(PROBER_DIR)

# Install the pre-push hook (lint + unit tests, see lefthook.yml).
.PHONY: hooks
hooks:
	lefthook install
