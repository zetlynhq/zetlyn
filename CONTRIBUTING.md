# Contributing

Thank you for wanting to make Zetlyn better. Issues and pull requests are welcome; for a larger
change, open an issue or a discussion first, so that we agree on what it should do before you
build it.

## How

- Run `cargo test` before sending a change; the tests need no network.
- A pull request says what it lets somebody do and how you tested it.
- Code reads like the code around it: comments say why, names say what, and the words are plain.
- Rust and POSIX shell only.
- When dependencies change, run `cargo about generate about.hbs -o THIRD_PARTY_LICENSES` and
  commit the result; a crate under a licence not in `about.toml` needs a word in the pull request.
- Security issues are not for pull requests or issues: see [SECURITY.md](SECURITY.md).

## Terms

By sending a contribution you agree that it is yours to give, and that it is licensed under the
MIT License, as the rest of Zetlyn is (see [LICENSE](LICENSE)). The name Zetlyn and its mark stay
with zetlyn.com (see [NOTICE](NOTICE)).

Everyone taking part follows the [Code of Conduct](CODE_OF_CONDUCT.md).
