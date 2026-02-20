.PHONY: build release release-patch release-minor run run-global test test-mocks test-verbose clippy fmt fmt-check lint check clean install loc help

# Default target
help: ## Show this help
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | sort | awk 'BEGIN {FS = ":.*?## "}; {printf "\033[36m%-20s\033[0m %s\n", $$1, $$2}'

# Build
build: ## Build debug binary
	cargo build

release: ## Build release binary
	cargo build --release

release-patch: ## Bump patch version (0.1.0 → 0.1.1) and build release
	@VERSION=$$(awk -F'"' '/^version/{print $$2; exit}' Cargo.toml); \
	MAJOR=$$(echo $$VERSION | cut -d. -f1); \
	MINOR=$$(echo $$VERSION | cut -d. -f2); \
	PATCH=$$(echo $$VERSION | cut -d. -f3); \
	NEW="$$MAJOR.$$MINOR.$$((PATCH + 1))"; \
	sed -i '' "s/^version = \"$$VERSION\"/version = \"$$NEW\"/" Cargo.toml; \
	echo "Version: $$VERSION → $$NEW"; \
	cargo build --release

release-minor: ## Bump minor version (0.1.0 → 0.2.0) and build release
	@VERSION=$$(awk -F'"' '/^version/{print $$2; exit}' Cargo.toml); \
	MAJOR=$$(echo $$VERSION | cut -d. -f1); \
	MINOR=$$(echo $$VERSION | cut -d. -f2); \
	NEW="$$MAJOR.$$((MINOR + 1)).0"; \
	sed -i '' "s/^version = \"$$VERSION\"/version = \"$$NEW\"/" Cargo.toml; \
	echo "Version: $$VERSION → $$NEW"; \
	cargo build --release

check: ## Type-check without building
	cargo check

# Run
run: build ## Build and run (project mode)
	cargo run

run-global: build ## Build and run in dashboard mode
	cargo run -- -g

# Test
test: ## Run all tests
	cargo test

test-mocks: ## Run tests with mock support
	cargo test --features test-mocks

test-verbose: ## Run tests with output
	cargo test -- --nocapture

# Lint & Format
clippy: ## Run clippy lints
	cargo clippy -- -D warnings

fmt: ## Format code
	cargo fmt

fmt-check: ## Check formatting without changes
	cargo fmt -- --check

lint: clippy fmt-check ## Run all lints (clippy + format check)

# Install
install: release ## Build release and install to ~/.local/bin
	@mkdir -p ~/.local/bin
	cp target/release/agtx ~/.local/bin/agtx
	@echo "Installed to ~/.local/bin/agtx"

# Utilities
clean: ## Clean build artifacts
	cargo clean

loc: ## Count lines of code
	@find src tests -name '*.rs' | xargs wc -l | tail -1 | awk '{print "Rust: " $$1 " lines"}'
	@find src tests -name '*.rs' | wc -l | awk '{print "Files: " $$1}'

# CI-like full check
ci: fmt-check clippy test ## Run full CI check (format + clippy + tests)
