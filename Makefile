# ====================================================================================
# Variables

## General
APP_NAME := completion

MAKEFLAGS     += --no-builtin-rules
MAKEFLAGS     += --warn-undefined-variables
.DEFAULT_GOAL := help

## Derived
VSIX := extension/completion-$(shell node -p "require('./extension/package.json').version").vsix

## Model serving (llama.cpp)
HF_REPO     ?= bartowski/Qwen2.5-Coder-1.5B-Instruct-GGUF
HF_FILE     ?= Qwen2.5-Coder-1.5B-Instruct-Q4_K_M.gguf
MODEL_PORT  ?= 8000
LLAMA_CACHE ?= $(HOME)/.cache/huggingface
MODEL_LOG   ?= /tmp/llama-server-$(MODEL_PORT).log

# ====================================================================================
# Colors

BLUE   := $(shell printf "\033[34m")
GREEN  := $(shell printf "\033[32m")
RED    := $(shell printf "\033[31m")
CYAN   := $(shell printf "\033[36m")
CNone  := $(shell printf "\033[0m")

# ====================================================================================
# Logger

TIME_SHORT = `date +%H:%M:%S`
INFO = @echo ${TIME_SHORT} ${BLUE}[ .. ]${CNone}
OK   = @echo ${TIME_SHORT} ${GREEN}[ OK ]${CNone}
FAIL = (echo ${TIME_SHORT} ${RED}[FAIL]${CNone} && false)

# ====================================================================================
# Help

help: ## show this help
	@echo "Usage:"
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) |\
	awk 'BEGIN {FS = ":.*?## "}; {printf "make ${CYAN}%-22s${CNone} %s\n", $$1, $$2}'

# ====================================================================================
# Extension

.PHONY: compile
compile: ## tsc + cargo build, stage the sidecar binary
	$(INFO) compiling extension
	cd extension && npm run compile
	cargo build --release --manifest-path sidecar/Cargo.toml
	mkdir -p extension/bin
	cp sidecar/target/release/sidecar extension/bin/
	$(OK) compiled

.PHONY: package
package: compile ## build the .vsix
	$(INFO) packaging
	cd extension && npx @vscode/vsce package
	$(OK) packaged

.PHONY: install
install: package ## install the extension into VS Code
	$(INFO) installing $(VSIX)
	code --install-extension $(VSIX)
	$(OK) installed

.PHONY: uninstall
uninstall: ## remove the extension
	$(INFO) uninstalling
	code --uninstall-extension vasilis.completion

.PHONY: clean
clean: ## drop staged artifacts
	$(INFO) cleaning
	rm -f extension/completion-*.vsix
	rm -f extension/bin/sidecar

# ====================================================================================
# Sidecar

.PHONY: sidecar-lint
sidecar-lint: ## clippy, fail on warnings + missing docs
	$(INFO) clippy
	cargo clippy --manifest-path sidecar/Cargo.toml --all-targets --all-features \
		-- -D warnings -W missing_docs || ${FAIL}
	$(OK) clippy

.PHONY: sidecar-lint-fix
sidecar-lint-fix: ## cargo fmt + clippy --fix
	$(INFO) clippy --fix
	cargo fmt --manifest-path sidecar/Cargo.toml
	cargo clippy --manifest-path sidecar/Cargo.toml --all-targets --all-features \
		--fix --allow-dirty --allow-staged \
		-- -D warnings -W missing_docs
	$(OK) clippy --fix

# ====================================================================================
# Telemetry

.PHONY: postgres-start
postgres-start: ## postgres:18 via docker-compose in the background
	$(INFO) postgres up
	docker compose up -d --wait || ${FAIL}
	$(OK) postgres up

.PHONY: postgres-stop
postgres-stop: ## stop postgres AND drop the named volume (wipes state)
	$(INFO) postgres down -v
	docker compose down -v

.PHONY: telemetry-compile
telemetry-setup: ## mix setup (create+migrate db)
	$(INFO) mix setup
	cd telemetry  && mix setup && mix compile || ${FAIL}
	$(OK) telemetry ready

.PHONY: telemetry
telemetry: ## run phx.server on :4000 (foreground)
	$(INFO) phx.server
	cd telemetry && mix phx.server

# ====================================================================================
# Model (llama.cpp)

.PHONY: model
model: ## llama-server on $(MODEL_PORT) (foreground)
	@command -v llama-server >/dev/null || { echo "llama-server not found. brew install llama.cpp"; exit 1; }
	$(INFO) llama-server :$(MODEL_PORT) $(HF_REPO)/$(HF_FILE)
	LLAMA_CACHE="$(LLAMA_CACHE)" llama-server \
		--hf-repo "$(HF_REPO)" \
		--hf-file "$(HF_FILE)" \
		--port "$(MODEL_PORT)" \
		--host 127.0.0.1 2>&1 | tee "$(MODEL_LOG)"

.PHONY: model-pull
model-pull: ## download the model into the HF cache
	@command -v llama-cli >/dev/null || { echo "llama-cli not found. brew install llama.cpp"; exit 1; }
	$(INFO) pulling $(HF_REPO)/$(HF_FILE)
	LLAMA_CACHE="$(LLAMA_CACHE)" llama-cli \
		--hf-repo "$(HF_REPO)" \
		--hf-file "$(HF_FILE)" \
		--prompt "" \
		--n-predict 1 \
		--no-warmup 2>&1 | tail -5

.PHONY: model-stop
model-stop: ## kill llama-server on $(MODEL_PORT)
	$(INFO) killing llama-server
	-pkill -f "llama-server.*--port $(MODEL_PORT)"

.PHONY: model-logs
model-logs: ## tail the llama-server log
	@tail -f "$(MODEL_LOG)"
