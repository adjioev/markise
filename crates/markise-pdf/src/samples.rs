//! Sample documents for export tests and the `export_samples` example (original text).
#![doc(hidden)]

/// A document exercising every block and inline type (front matter with header/footer).
pub const SAMPLE: &str = r#"---
title: Field Notes on Lighthouses
author: Ada Keeper
header: ${title} — ${author}
footer: Page ${pageNo} of ${totalPages} · ${today}
---

[toc]

# Field Notes on Lighthouses

A lighthouse is a tower that guides ships at night. This page exercises **strong text**,
*emphasis*, `inline code`, ~~struck words~~, <mark>highlighted words</mark>, <u>underlined</u>,
H<sub>2</sub>O, x<sup>2</sup>, <kbd>Ctrl</kbd>+<kbd>S</kbd> and a [link to the
harbour office](https://example.org/harbour). Jump to [the lamp table](#lamp-types) or read
the footnote[^fog]. Inline math: $e^{i\pi}+1=0$.

## Keepers and Lamps

> The lamp was trimmed at dusk and the log written at dawn.
> — from an imaginary keeper's diary
>
> > A nested quote with a second paragraph.

> [!WARNING]
> Never look straight into a lit lens.

### Duties

- Trim the wick and polish the lens
- Wind the clockwork
  - every four hours
  - more often in a storm
- [x] Record the weather
- [ ] Repaint the gallery rail

1. Light the lamp
2. Check the fog signal
3. Write the log

#### Lamp types

| Lamp | Fuel | Range (nmi) |
|:-----|:----:|------------:|
| Argand | colza oil | 12 |
| Pressure | kerosene, with a long description that has to wrap inside its cell | 18 |
| Electric | mains | 25 |

![A lens](lens.png "Fresnel lens")

```rust
fn beam(angle: f32) -> bool {
    // one flash every ten seconds
    (angle % 360.0) < 12.0
}
```

$$
r = 2.08 \left(\sqrt{h} + \sqrt{5}\right)
$$

---

## Logbook

Entry 1. The wind came from the north at 3 knots; visibility was moderate. The lamp burned steadily and the clockwork was wound four times during the watch, which is more than the regulations ask for but less than the storm demanded.

Entry 2. The wind came from the east at 4 knots; visibility was poor. The lamp burned steadily and the clockwork was wound four times during the watch, which is more than the regulations ask for but less than the storm demanded.

Entry 3. The wind came from the south at 5 knots; visibility was good. The lamp burned steadily and the clockwork was wound four times during the watch, which is more than the regulations ask for but less than the storm demanded.

Entry 4. The wind came from the west at 6 knots; visibility was moderate. The lamp burned steadily and the clockwork was wound four times during the watch, which is more than the regulations ask for but less than the storm demanded.

Entry 5. The wind came from the north-west at 7 knots; visibility was poor. The lamp burned steadily and the clockwork was wound four times during the watch, which is more than the regulations ask for but less than the storm demanded.

Entry 6. The wind came from the south-east at 8 knots; visibility was good. The lamp burned steadily and the clockwork was wound four times during the watch, which is more than the regulations ask for but less than the storm demanded.

Entry 7. The wind came from the north at 2 knots; visibility was moderate. The lamp burned steadily and the clockwork was wound four times during the watch, which is more than the regulations ask for but less than the storm demanded.

Entry 8. The wind came from the east at 3 knots; visibility was poor. The lamp burned steadily and the clockwork was wound four times during the watch, which is more than the regulations ask for but less than the storm demanded.

Entry 9. The wind came from the south at 4 knots; visibility was good. The lamp burned steadily and the clockwork was wound four times during the watch, which is more than the regulations ask for but less than the storm demanded.

Entry 10. The wind came from the west at 5 knots; visibility was moderate. The lamp burned steadily and the clockwork was wound four times during the watch, which is more than the regulations ask for but less than the storm demanded.

Entry 11. The wind came from the north-west at 6 knots; visibility was poor. The lamp burned steadily and the clockwork was wound four times during the watch, which is more than the regulations ask for but less than the storm demanded.

Entry 12. The wind came from the south-east at 7 knots; visibility was good. The lamp burned steadily and the clockwork was wound four times during the watch, which is more than the regulations ask for but less than the storm demanded.

Entry 13. The wind came from the north at 8 knots; visibility was moderate. The lamp burned steadily and the clockwork was wound four times during the watch, which is more than the regulations ask for but less than the storm demanded.

Entry 14. The wind came from the east at 2 knots; visibility was poor. The lamp burned steadily and the clockwork was wound four times during the watch, which is more than the regulations ask for but less than the storm demanded.

Entry 15. The wind came from the south at 3 knots; visibility was good. The lamp burned steadily and the clockwork was wound four times during the watch, which is more than the regulations ask for but less than the storm demanded.

Entry 16. The wind came from the west at 4 knots; visibility was moderate. The lamp burned steadily and the clockwork was wound four times during the watch, which is more than the regulations ask for but less than the storm demanded.

Entry 17. The wind came from the north-west at 5 knots; visibility was poor. The lamp burned steadily and the clockwork was wound four times during the watch, which is more than the regulations ask for but less than the storm demanded.

Entry 18. The wind came from the south-east at 6 knots; visibility was good. The lamp burned steadily and the clockwork was wound four times during the watch, which is more than the regulations ask for but less than the storm demanded.

Entry 19. The wind came from the north at 7 knots; visibility was moderate. The lamp burned steadily and the clockwork was wound four times during the watch, which is more than the regulations ask for but less than the storm demanded.

Entry 20. The wind came from the east at 8 knots; visibility was poor. The lamp burned steadily and the clockwork was wound four times during the watch, which is more than the regulations ask for but less than the storm demanded.

Entry 21. The wind came from the south at 2 knots; visibility was good. The lamp burned steadily and the clockwork was wound four times during the watch, which is more than the regulations ask for but less than the storm demanded.

Entry 22. The wind came from the west at 3 knots; visibility was moderate. The lamp burned steadily and the clockwork was wound four times during the watch, which is more than the regulations ask for but less than the storm demanded.

Entry 23. The wind came from the north-west at 4 knots; visibility was poor. The lamp burned steadily and the clockwork was wound four times during the watch, which is more than the regulations ask for but less than the storm demanded.

Entry 24. The wind came from the south-east at 5 knots; visibility was good. The lamp burned steadily and the clockwork was wound four times during the watch, which is more than the regulations ask for but less than the storm demanded.

Entry 25. The wind came from the north at 6 knots; visibility was moderate. The lamp burned steadily and the clockwork was wound four times during the watch, which is more than the regulations ask for but less than the storm demanded.

Entry 26. The wind came from the east at 7 knots; visibility was poor. The lamp burned steadily and the clockwork was wound four times during the watch, which is more than the regulations ask for but less than the storm demanded.

Entry 27. The wind came from the south at 8 knots; visibility was good. The lamp burned steadily and the clockwork was wound four times during the watch, which is more than the regulations ask for but less than the storm demanded.

Entry 28. The wind came from the west at 2 knots; visibility was moderate. The lamp burned steadily and the clockwork was wound four times during the watch, which is more than the regulations ask for but less than the storm demanded.

Entry 29. The wind came from the north-west at 3 knots; visibility was poor. The lamp burned steadily and the clockwork was wound four times during the watch, which is more than the regulations ask for but less than the storm demanded.

Entry 30. The wind came from the south-east at 4 knots; visibility was good. The lamp burned steadily and the clockwork was wound four times during the watch, which is more than the regulations ask for but less than the storm demanded.

##### A smaller heading

###### The smallest heading

Unicode check: Привет, мир — Ελληνικά — 日本語のテキスト — emoji 🌊⚓.

[^fog]: Fog signals were sounded every thirty seconds in thick weather.
"#;

/// Code blocks in several languages (highlighting check).
pub const CODE_SAMPLES: &str = r#"## Code samples

```rust
/// A lamp with a flash pattern.
#[derive(Debug, Clone)]
pub struct Lamp<'a> { name: &'a str, period: f32 }

impl Lamp<'_> {
    pub fn is_lit(&self, t: f32) -> bool {
        (t % self.period) < 0.5 // short flash
    }
}
```

```ts
interface Beacon { id: number; label?: string }
export async function load(id: number): Promise<Beacon> {
  const res = await fetch(`/beacons/${id}`);
  return (await res.json()) as Beacon; // trust the server
}
```

```python
import math

class Tower:
    """A lighthouse tower."""
    def range_nmi(self, height_m: float) -> float:
        # geographic range for a 5 m eye height
        return 2.08 * (math.sqrt(height_m) + math.sqrt(5.0))
```

```json
{"name": "Fastnet", "lit": 1904, "range": 27.5, "automated": true, "keeper": null}
```

```bash
#!/usr/bin/env bash
for f in logs/*.txt; do
  if grep -q "fog" "$f"; then echo "fog in ${f##*/}"; fi
done
```

```yaml
lighthouse:
  name: Fastnet   # Ireland
  height: 54
  lamps: [main, reserve]
```
"#;
