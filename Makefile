install:
	mkdir -p $(HOME)/.local/bin
	cargo install --path . --locked --root $(HOME)/.local

install-tools:
	uv tool install ty
	uv tool install ruff

test:
	cargo test --locked

# Regenerate THIRD_PARTY_LICENSES.md after changing dependencies; CI checks it's current.
licenses:
	cargo about generate --fail about.hbs > THIRD_PARTY_LICENSES.md
