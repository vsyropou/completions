VSIX := vscode-extension/completion-$(shell node -p "require('./vscode-extension/package.json').version").vsix

.PHONY: compile package install uninstall clean

compile:
	cd vscode-extension && npm run compile
	cargo build --release --manifest-path sidecar/Cargo.toml
	mkdir -p vscode-extension/bin
	cp sidecar/target/release/sidecar vscode-extension/bin/

package: compile
	cd vscode-extension && npx @vscode/vsce package

install: package
	code --install-extension $(VSIX)

uninstall:
	code --uninstall-extension vasilis.completion

clean:
	rm -f vscode-extension/completion-*.vsix
	rm -f vscode-extension/bin/sidecar
