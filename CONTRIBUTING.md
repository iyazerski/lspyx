# Contributing

Thanks for helping with lspyx. Bug reports, ideas and pull requests are all welcome.

## Report a bug or ask for a feature

[Open an issue](https://github.com/iyazerski/lspyx/issues/new/choose) and pick a template. For questions, use [Discussions](https://github.com/iyazerski/lspyx/discussions). For security problems, follow [SECURITY.md](SECURITY.md) instead.

## Make a change

You need macOS or Linux, [Rust](https://rustup.rs), and [ty](https://github.com/astral-sh/ty) and [Ruff](https://github.com/astral-sh/ruff) on your `PATH` to try changes on a real project (`make install-tools` installs them with uv).

```sh
make install   # build and put lspyx in ~/.local/bin
make test
```

A running daemon keeps the old binary. Run `lspyx daemon stop` in your test project after each install.

Before you open a pull request:

1. Install the hooks with [prek](https://github.com/j178/prek): `prek install`. They run `cargo fmt`, `cargo check` and `cargo clippy`.
2. Run `make test`.
3. If you changed dependencies, run `make licenses` and commit `THIRD_PARTY_LICENSES.md`. It needs [cargo-about](https://github.com/EmbarkStudios/cargo-about).
4. Keep pull requests small and focused on one change.

## Code style

- Follow the code next to your change.
- Give every item a doc comment, and every `unsafe` block a `// SAFETY:` comment.
- Use comments to say why, not what.
- Keep tool output short: agents pay for every line.

## Releases

Maintainers release by bumping `version` in `Cargo.toml` and pushing a matching `vX.Y.Z` tag. The [release workflow](.github/workflows/release.yml) builds the binaries and updates GitHub Releases, Homebrew and crates.io.

By contributing, you agree that your work is released under the [MIT License](LICENSE). Please follow the [Code of Conduct](CODE_OF_CONDUCT.md).
