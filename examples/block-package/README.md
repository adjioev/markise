# Example block package

A complete block package for a made-up recipe site, with a sample document that uses it. Original
work under the repository's licence; copy it as the start of your own package. The format is
described in [Making block packages](../../docs/user/block-packages.md).

| Path | What it is |
|---|---|
| `package/` | the package's source: manifest, four blocks, palette, views, icons, styles and a script |
| `document/` | a document that uses all four blocks, and its picture |

The blocks show the ways a block can look:

| Block | Native view | Site's code |
|---|---|---|
| `tip` | `callout` | styles only, with a `warning` variant |
| `ingredients` | `card` | styles only; its argument says how many it serves |
| `timing` | none, so it shows its picture | styles and a script that draws a bar |
| `question` | `choices`, which can be [practised](../../docs/user/blocks.md#practice) | styles only |

## Try it

```sh
cargo xtask block-package
cargo run -p markise --release -- target/block-package-example/weeknight-loaf.md
```

The first command packs `package/` into `blocks/recipe-site-1.0.0.tar.gz` and writes the document
and a `blocks.config.json` that pins the archive's hash into `target/block-package-example/`
(`--out FOLDER` chooses another place). The archive and binding are built, not committed, so the
hash always belongs to these files. Markise asks before it uses the package.

A test holds all of this to the real code: `cargo test -p markise-web-helper --test it
example_package::` validates the package and reads the sample document on every system, and on
macOS draws each block with the real helper.
