# Security

lspyx runs on your machine with your permissions. It reads your code, starts ty and Ruff, and runs a daemon that listens on a Unix socket in `~/.cache/lspyx`. Please report anything that could let someone else misuse that.

## Report a vulnerability

Report it privately through [GitHub's vulnerability reporting](https://github.com/iyazerski/lspyx/security/advisories/new). Please don't open a public issue.

Include what you found, how to reproduce it, and the lspyx version (`lspyx --version`). You'll get an answer within a week.

## Supported versions

Only the latest release gets fixes. Update with `lspyx update`, `brew upgrade lspyx` or `cargo install lspyx --locked`.

## How releases are protected

- Every release is built by [GitHub Actions](.github/workflows/release.yml) from the tagged public source, after CI passes.
- Every archive has a SHA-256 checksum next to it. `install.sh` and `lspyx update` refuse a download that doesn't match it, and Homebrew checks the checksum in its formula.
- The binaries aren't code-signed or notarized.
