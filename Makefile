install:
	mkdir -p $(HOME)/.local/bin
	cargo install --path . --locked --root $(HOME)/.local

install-tools:
	uv tool install ty
	uv tool install ruff

test:
	cargo test
