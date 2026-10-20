VSIX := extension/completion-$(shell node -p "require('./extension/package.json').version").vsix

## Inference hosting via llama.cpp ---------------------------------------------

# Repo + file on Hugging Face. Qwen2.5-Coder-Instruct ships GGUFs under both
# Qwen's official account (lowercase filenames) and bartowski's (CamelCase).
HF_REPO  ?= bartowski/Qwen2.5-Coder-1.5B-Instruct-GGUF
HF_FILE  ?= Qwen2.5-Coder-1.5B-Instruct-Q4_K_M.gguf
MODEL_PORT ?= 8000
LLAMA_CACHE ?= $(HOME)/.cache/huggingface

.PHONY: compile package install uninstall clean model model-pull model-stop model-logs

compile:
	cd extension && npm run compile
	cargo build --release --manifest-path sidecar/Cargo.toml
	mkdir -p extension/bin
	cp sidecar/target/release/sidecar extension/bin/

package: compile
	cd extension && npx @vscode/vsce package

install: package
	code --install-extension $(VSIX)

uninstall:
	code --uninstall-extension vasilis.completion

clean:
	rm -f extension/completion-*.vsix
	rm -f extension/bin/sidecar

model:
	@command -v llama-server >/dev/null || { echo "llama-server not found. brew install llama.cpp"; exit 1; }
	@echo "Starting llama-server on port $(MODEL_PORT), repo=$(HF_REPO), file=$(HF_FILE)"
	@echo "Cache: $(LLAMA_CACHE)  (set LLAMA_CACHE env var to change)"
	LLAMA_CACHE="$(LLAMA_CACHE)" llama-server \
		--hf-repo "$(HF_REPO)" \
		--hf-file "$(HF_FILE)" \
		--port "$(MODEL_PORT)" \
		--host 127.0.0.1

# Download the model file without starting the server (first-time warm-up).
model-pull:
	@command -v llama-cli >/dev/null || { echo "llama-cli not found. brew install llama.cpp"; exit 1; }
	LLAMA_CACHE="$(LLAMA_CACHE)" llama-cli \
		--hf-repo "$(HF_REPO)" \
		--hf-file "$(HF_FILE)" \
		--prompt "" \
		--n-predict 1 \
		--no-conversation 2>&1 | tail -5

model-stop:
	-pkill -f "llama-server.*--port $(MODEL_PORT)"

model-logs:
	@lsof -i :$(MODEL_PORT) -sTCP:LISTEN -n -P 2>/dev/null | tail -n +2 | awk '{print $$1, $$2}' || echo "no llama-server on port $(MODEL_PORT)"
