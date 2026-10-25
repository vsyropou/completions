VSIX := extension/completion-$(shell node -p "require('./extension/package.json').version").vsix

## Inference hosting via llama.cpp ---------------------------------------------

# Repo + file on Hugging Face. Qwen2.5-Coder-Instruct ships GGUFs under both
# Qwen's official account (lowercase filenames) and bartowski's (CamelCase).
HF_REPO  ?= bartowski/Qwen2.5-Coder-1.5B-Instruct-GGUF
HF_FILE  ?= Qwen2.5-Coder-1.5B-Instruct-Q4_K_M.gguf
MODEL_PORT ?= 8000
LLAMA_CACHE ?= $(HOME)/.cache/huggingface
MODEL_LOG ?= /tmp/llama-server-$(MODEL_PORT).log

# extention targets
# .PHONY: compile
compile:
	cd extension && npm run compile
	cargo build --release --manifest-path sidecar/Cargo.toml
	mkdir -p extension/bin
	cp sidecar/target/release/sidecar extension/bin/

.PHONY: package
package: compile
	cd extension && npx @vscode/vsce package

.PHONY: install
install: package
	code --install-extension $(VSIX)

.PHONY: uninstall
uninstall:
	code --uninstall-extension vasilis.completion

.PHONY: clean
clean:
	rm -f extension/completion-*.vsix
	rm -f extension/bin/sidecar

# Sidecar lint — ruff-style. `lint` reports only; `lint-fix` applies clippy
# suggestions + cargo fmt in-place (≈ `ruff check --fix`).
.PHONY: sidecar-lint
sidecar-lint:
	cargo clippy --manifest-path sidecar/Cargo.toml --all-targets --all-features \
		-- -D warnings -W missing_docs

.PHONY: sidecar-lint-fix
sidecar-lint-fix:
	cargo fmt --manifest-path sidecar/Cargo.toml
	cargo clippy --manifest-path sidecar/Cargo.toml --all-targets --all-features \
		--fix --allow-dirty --allow-staged \
		-- -D warnings -W missing_docs

# telemetry targets
.PHONY: compile-telemetry
compile-telemetry:
	cd telemetry && mix deps.get && mix compile

# model targets
.PHONY: model
model:
	@command -v llama-server >/dev/null || { echo "llama-server not found. brew install llama.cpp"; exit 1; }
	@echo "Starting llama-server on port $(MODEL_PORT), repo=$(HF_REPO), file=$(HF_FILE)"
	@echo "Cache: $(LLAMA_CACHE)  (set LLAMA_CACHE env var to change)"
	@echo "Logs: $(MODEL_LOG)  (tail -f, or 'make model-logs')"
	LLAMA_CACHE="$(LLAMA_CACHE)" llama-server \
		--hf-repo "$(HF_REPO)" \
		--hf-file "$(HF_FILE)" \
		--port "$(MODEL_PORT)" \
		--host 127.0.0.1 2>&1 | tee "$(MODEL_LOG)"

.PHONY: model-pull
# Download the model file without starting the server (first-time warm-up).
model-pull:
	@command -v llama-cli >/dev/null || { echo "llama-cli not found. brew install llama.cpp"; exit 1; }
	LLAMA_CACHE="$(LLAMA_CACHE)" llama-cli \
		--hf-repo "$(HF_REPO)" \
		--hf-file "$(HF_FILE)" \
		--prompt "" \
		--n-predict 1 \
		--no-warmup 2>&1 | tail -5

.PHONY: model-stop
model-stop:
	-pkill -f "llama-server.*--port $(MODEL_PORT)"

.PHONY: model-logs
model-logs:
	@tail -f "$(MODEL_LOG)"
