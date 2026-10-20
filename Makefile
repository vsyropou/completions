VSIX := extension/completion-$(shell node -p "require('./extension/package.json').version").vsix

.PHONY: compile package install uninstall clean

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
