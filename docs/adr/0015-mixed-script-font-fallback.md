# ADR 0015: Mixed-script font fallback

Status: accepted for implementation (issue #92).

## Problem and evidence

The editor's default epaint backend previously left face selection to character fallback.
A theme face covering Latin could therefore render English words beside unrelated Cyrillic
letterforms. Theme roles also dropped the system body/bold fallback once any theme face had
resolved. A deterministic test with a Latin-only face and a bilingual face reproduced the
mismatch (6-point versus 12-point advances). A Phycat screenshot reproduced the same visible
mixing with installed system fonts.

## Decision

The layout text adapters choose a face covering the letters, digits and combining marks of
the shaped paragraph before falling back to words. This keeps adjacent English words with
Cyrillic, Greek or CJK text when a common face is available, including across bold/italic
sections. Roles retain separate regular/bold and monospace chains. If no face covers the
paragraph, selection works on Unicode words; if a word itself has no covering face, the
backend's existing cluster fallback remains. Emoji and punctuation retain the rest of the
chain. ASCII-only text follows the original path.

A covering primary face stays preferred. Other covering faces are ranked by available OS/2
serif/sans classification, weight and normalized x-height, with stack order breaking ties.
Unknown classification does not invent a font category. Fallback point size is adjusted to
the primary face's x-height (OS/2, or the x glyph bounds when absent), bounded to 0.75–1.35.
This changes both glyph size and advance; it is not a visual-only raster tweak. The configured
line box stays the same. Existing theme line-height choices are retained. Real bold faces
are preferred where available; this does not add epaint synthetic bold or change its existing
italic handling.

The same coverage/metric policy drives epaint and the optional parley adapter. Definitions
register lightweight named chains for explicit face choices; font bytes remain shared.
Coverage and metric helpers live in GUI-free `markise-text`; adapter-specific family handling
lives in `markise-layout`. Installed-font discovery and file reads remain on the theme worker.
No full-document scan, new dependency or bundled font is introduced.

Preferences → Appearance adds Cyrillic, Greek and CJK family overrides. An override applies
to prose/headings containing that script and adjacent Latin words if it covers them; code
keeps its monospace stack. Empty values restore automatic fallback. These are editor font
choices, not source edits or new CSS export rules. Values persist and participate independently
in settings patch merge, recovery and reset. Missing fonts keep the existing theme-report
behavior and automatic fallback.

## Validation

Synthetic-font shaping tests exercise both adapters, mixed scripts, x-height/line boxes,
serif matching, bold/italic and overrides. Preferences tests enter, save and clear a script
font. Settings tests cover serialization, independent patch merge, recovery and clearing.
Screenshots compare RU+EN, Greek+EN and CJK+EN, with bold/italic, in Markise Default, Drake and
Phycat in light/dark on macOS. Actual Windows/Linux installed-font appearance remains a
platform check; synthetic coverage tests do not depend on installed fonts.

Local release Criterion measurement (2026-11-01, 20 samples): changing and shaping a
1,000-character RU+EN paragraph with a partial-coverage primary takes about 110 microseconds
(107–112 µs interval), below the 16 ms interactive budget. This is the shaping component,
not an end-to-end keystroke latency measurement.
