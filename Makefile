CARGO   ?= cargo
BIN     := diskdiet
PATHS   ?= $(HOME)
VERSION := $(shell sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)

.DEFAULT_GOAL := help

.PHONY: help build release install install-gui uninstall scan diet tui big docker dry-run stats \
        gui gui-release app test lint fmt service-install service-uninstall service-status clean

help: ## Show this help
	@echo "diskdiet $(VERSION) - put your storage on a diet"
	@echo
	@grep -E '^[a-zA-Z_-]+:.*?## ' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*?## "}; {printf "  \033[36m%-18s\033[0m %s\n", $$1, $$2}'
	@echo
	@echo "  Override the search folders with PATHS, e.g. make diet PATHS=~/code"

## ---- Build & install ----------------------------------------------------

build: ## Debug build (CLI)
	$(CARGO) build

release: ## Optimized CLI build (target/release/diskdiet)
	$(CARGO) build --release

install: ## Install the CLI into ~/.cargo/bin
	$(CARGO) install --path . --locked

install-gui: ## Install CLI + desktop app into ~/.cargo/bin
	$(CARGO) install --path . --locked --features gui

uninstall: ## Remove diskdiet from ~/.cargo/bin
	$(CARGO) uninstall $(BIN)

## ---- Use -----------------------------------------------------------------

scan: ## Show what can be cleaned (read-only)
	$(CARGO) run --release --quiet -- scan $(PATHS)

diet: ## Interactively pick what to delete (numbered picker)
	$(CARGO) run --release --quiet -- clean $(PATHS)

tui: ## Interactively pick what to delete (full-screen picker)
	$(CARGO) run --release --quiet -- clean --tui $(PATHS)

big: ## Find big forgotten files in Downloads, Desktop, Documents, Movies
	$(CARGO) run --release --quiet -- big

dry-run: ## Show what `clean --yes` would delete
	$(CARGO) run --release --quiet -- clean --dry-run $(PATHS)

docker: ## Pick Docker images / containers / volumes to remove
	$(CARGO) run --release --quiet -- docker

stats: ## Show how much space diskdiet has freed
	$(CARGO) run --release --quiet -- stats

## ---- Desktop app -----------------------------------------------------------

gui: ## Run the desktop app
	$(CARGO) run --release --features gui --bin diskdiet-gui

gui-release: ## Build the desktop app (target/release/diskdiet-gui)
	$(CARGO) build --release --features gui --bin diskdiet-gui

app: gui-release ## Build dist/diskdiet.app (macOS)
	scripts/bundle-macos.sh target/release/diskdiet-gui dist $(VERSION)

## ---- Develop ---------------------------------------------------------------

test: ## Run tests
	$(CARGO) test --all-features

lint: ## Run clippy and check formatting
	$(CARGO) clippy --all-targets --all-features -- -D warnings
	$(CARGO) fmt --check

fmt: ## Format code
	$(CARGO) fmt

## ---- Background service ----------------------------------------------------

service-install: install ## Weekly cleanup of projects untouched for 14d + alert below 20GB
	$(BIN) service install $(PATHS) --alert-below 20GB

service-uninstall: ## Remove the scheduled jobs
	$(BIN) service uninstall

service-status: ## Show scheduled job status
	$(BIN) service status

clean: ## Remove this repo's own build artifacts (ironic, we know)
	$(CARGO) clean
	rm -rf dist
