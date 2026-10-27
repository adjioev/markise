# KaTeX fonts (math rendering)

- Upstream: KaTeX, https://github.com/KaTeX/KaTeX — files taken unchanged from the npm release
  `katex@0.16.22` (`dist/fonts/*.ttf`, git head `613c3da85b083f602c0d5265da0f735ddb728dbb`,
  fetched 2026-10-27). The fonts are generated from the katex-fonts project
  (https://github.com/KaTeX/katex-fonts), originally based on the MathJax TeX fonts.
- License: SIL Open Font License 1.1 — see [`OFL.txt`](OFL.txt). Copyright (c) 2009-2010 Design
  Science, Inc.; Copyright (c) 2014-2018 Khan Academy. OFL-1.1 is allowed for bundled font files
  only (owner decision 2026-10-27, AGENTS.md). Listed in the repository `NOTICE`.
- Reserved Font Names: every family (`KaTeX_AMS`, `KaTeX_Main`, …) is a Reserved Font Name. The
  files are therefore shipped **unmodified** (no subsetting, no renaming); a modified copy would
  have to be renamed.
- Why these fonts: the math layout engine (RaTeX, `ratex-layout`, MIT) carries the KaTeX font
  metrics, so the glyph outlines must come from exactly these files (docs/adr/0008).
- Size: 19 files (KaTeX_Caligraphic-Bold is not used by the layout engine and is not shipped), 490 KiB (501 296 bytes) in total, embedded by `crates/markise-blocks/src/math/fonts.rs`.

Files (SHA-256):

| File | SHA-256 |
|---|---|
| `KaTeX_AMS-Regular.ttf` | `68534840bcfdd2bffb6f0e8deb48684dd01e7f04ea2813267577afb906de1d13` |
| `KaTeX_Caligraphic-Regular.ttf` | `ed0b74372feefcbb9c0666b2e210da37b7e49fa7fbbf3eeb11db5f693dacfbb7` |
| `KaTeX_Fraktur-Bold.ttf` | `9163df9c7122432e6495b4229fa9071cf9ae86a758ae5efc4924ec2e1a6dbce1` |
| `KaTeX_Fraktur-Regular.ttf` | `1e6f9579e90e2cac37f8f60a597c436e075c114385652b7cbeb0dec0421291b3` |
| `KaTeX_Main-Bold.ttf` | `138ac28d1663b3037e9c5f52371fa5c63d8324f4a38d22cd573e6ea3a3fd0cf8` |
| `KaTeX_Main-BoldItalic.ttf` | `70ee1f64a20f2048c21940ef46d0144fd215baa953ca69afd1e31e98544f708f` |
| `KaTeX_Main-Italic.ttf` | `0d85ae7cc30f23790a7f1a58c4a112fdca8aae769b6ba11429af1d98b1b6cb3a` |
| `KaTeX_Main-Regular.ttf` | `d0332f52868370fd83ae7fa46470f90c8f2eab2fcf12bc4f88080b340c95a830` |
| `KaTeX_Math-BoldItalic.ttf` | `f9377ab0271cda59af24bcffbd46a4d0c8a3572ffafdbb38de2ad5ea7b0d5ee5` |
| `KaTeX_Math-Italic.ttf` | `08ce98e51b04d58945a301e639e02b6998af29fdfd61a7b8afdd07bbfc479d4a` |
| `KaTeX_SansSerif-Bold.ttf` | `1ece03f79f95277d57dc7f6b435a74e1379b0d46104a8530286b60ff49369ea0` |
| `KaTeX_SansSerif-Italic.ttf` | `3931dd81faed86ba021bb2bbdc36f5bed9a38d6b4f4077aca59b265aa1b02083` |
| `KaTeX_SansSerif-Regular.ttf` | `f36ea897e19f4a2e571d1e900e4e3710e438deb05a842486045ba0a3e616a4ad` |
| `KaTeX_Script-Regular.ttf` | `1c67f068fea8bb09bf099c088b1cf64bd27516a6e07f4684344873564bb66a67` |
| `KaTeX_Size1-Regular.ttf` | `95b6d2f1a50173bfedb8c63e1d1c99b10427d0a4df4201cb44513b226951a22b` |
| `KaTeX_Size2-Regular.ttf` | `a6b2099fb555c60e3a0db3a08842ebf1d732c6eb4e4bf44913613bed4fc4e39b` |
| `KaTeX_Size3-Regular.ttf` | `500e04d54f0d51666332c9d2089aa803be22aa878eca539e59fa53c6e522b082` |
| `KaTeX_Size4-Regular.ttf` | `c647367d1dd4e162468717d020e1fc0f1dc5c26ebfdffbe55261713bf88c5877` |
| `KaTeX_Typewriter-Regular.ttf` | `f01f3e87d9c6a61c0c081ceb577abd864eb00a612f7ac1620dd6915fad2ef5aa` |
