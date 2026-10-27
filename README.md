# Markise

*The marquise of Markdown editors.*

Markise (pronounced like *marquise*, mar-KEEZ) is a fast, cross-platform live-preview Markdown
editor written in pure Rust, in the spirit of Typora. It is an independent clean-room
implementation; some editing behaviours were informed by MarkText.

**Status: beta.** Viewer, themes (styles × colour schemes, CSS theme import), code highlighting,
math (TeX), Mermaid diagrams, export (HTML, PDF, PNG, Pandoc formats), side-by-side source editing
and editing in the rendered view. Opens 10 MB documents in well under a second.

```sh
cargo run -p markise --release -- FILE.md|FOLDER          # desktop app (egui)
cargo run -p markise-cli -- html FILE.md -o FILE.html    # HTML export from the command line
cargo run -p markise-cli -- roundtrip FILE.md            # byte-identical round-trip check
cargo xtask ci                                           # fmt, clippy, tests, layer and asset checks
```

Design decisions live in [`docs/adr/`](docs/adr/), CI policy in [`docs/ci.md`](docs/ci.md), release
process in [`docs/release/`](docs/release/). Contributor and agent instructions: [AGENTS.md](AGENTS.md).

Bug reports and ideas: GitHub issues. Pull requests are welcome; they are reviewed before merging.

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.
Third-party attributions: [NOTICE](NOTICE).
