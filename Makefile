CARGO ?= cargo
BIN   := diskdiet
PATHS ?= $(HOME)

.DEFAULT_GOAL := help

.PHONY: help build release install uninstall scan diet docker dry-run test lint fmt service-install service-uninstall service-status clean

help: ## Show this help
	@echo "diskdiet - put your disk on a diet"
	@echo
	@grep -E '^[a-zA-Z_-]+:.*?## ' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*?## "}; {printf "  \033[36m%-18s\033[0m %s\n", $$1, $$2}'
	@echo
	@echo "  Override the search folders with PATHS, e.g. make diet PATHS=~/projects"

build: ## Debug build
	$(CARGO) build

release: ## Optimized build (target/release/diskdiet)
	$(CARGO) build --release

install: ## Install diskdiet into ~/.cargo/bin
	$(CARGO) install --path . --locked

uninstall: ## Remove diskdiet from ~/.cargo/bin
	$(CARGO) uninstall $(BIN)

scan: ## Show what can be cleaned (read-only)
	$(CARGO) run --release --quiet -- scan $(PATHS)

diet: ## Interactively pick what to delete
	$(CARGO) run --release --quiet -- clean $(PATHS)

dry-run: ## Show what `clean --yes` would delete
	$(CARGO) run --release --quiet -- clean --dry-run $(PATHS)

docker: ## Show Docker usage and prune it
	$(CARGO) run --release --quiet -- docker

test: ## Run tests
	$(CARGO) test

lint: ## Run clippy and check formatting
	$(CARGO) clippy --all-targets -- -D warnings
	$(CARGO) fmt --check

fmt: ## Format code
	$(CARGO) fmt

service-install: install ## Install + schedule weekly cleanup of projects untouched for 14 days
	$(BIN) service install $(PATHS)

service-uninstall: ## Remove the scheduled cleanup
	$(BIN) service uninstall

service-status: ## Show scheduled cleanup status
	$(BIN) service status

clean: ## Remove this repo's own build artifacts (ironic, we know)
	$(CARGO) clean
