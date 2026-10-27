//! CSS Fonts 4 §5.2 face-set selection on descriptors only (never on coverage), ADR 0005 §4.1
//! step 2. Pure functions over [`FaceDescriptors`]; the store applies them per family.

use crate::style::Slope;

/// `font-style` descriptor of an `@font-face` rule (or of a system face).
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum FaceSlope {
    #[default]
    Normal,
    Italic,
    /// Oblique angle range in degrees (`oblique 10deg 20deg`; a single angle has lo == hi).
    Oblique(f32, f32),
}

/// The descriptors CSS matching looks at.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FaceDescriptors {
    /// `font-weight` range (single weight: lo == hi).
    pub weight: (f32, f32),
    pub slope: FaceSlope,
    /// `font-stretch` percentage range.
    pub stretch: (f32, f32),
}

impl Default for FaceDescriptors {
    fn default() -> Self {
        FaceDescriptors {
            weight: (400.0, 400.0),
            slope: FaceSlope::Normal,
            stretch: (100.0, 100.0),
        }
    }
}

/// The requested font properties of a run.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Request {
    pub weight: f32,
    pub slope: Slope,
    pub stretch: f32,
}

/// Result of face-set selection.
#[derive(Clone, Debug, PartialEq)]
pub struct Selection {
    /// Indices (into the input slice) of the faces sharing the winning descriptor values —
    /// the unicode-range composite face — in declaration order.
    pub faces: Vec<usize>,
    /// The weight to use with the winning faces: the request clamped into their range (for
    /// variable instances, `wght`).
    pub weight: f32,
    /// The stretch to use (request clamped into the winning range; `wdth` for variable faces).
    pub stretch: f32,
    /// The oblique angle the winning faces matched at (`slnt` = −angle for variable faces);
    /// `None` for italic/normal matches.
    pub oblique: Option<f32>,
    /// Synthetic bold is allowed: the request is ≥ 600 and the matched weight ≤ 500.
    pub synthesize_bold: bool,
    /// Synthetic slant is allowed: italic/oblique was requested, the matched set is normal.
    pub synthesize_slant: bool,
}

type Key = (u8, f32);

fn keep_min(cands: Vec<usize>, key: impl Fn(usize) -> Key) -> Vec<usize> {
    let mut best: Option<Key> = None;
    for &i in &cands {
        let k = key(i);
        best = Some(match best {
            Some(b) if (b.0, b.1) <= (k.0, k.1) => b,
            _ => k,
        });
    }
    let Some(best) = best else { return cands };
    cands
        .into_iter()
        .filter(|&i| {
            let k = key(i);
            k.0 == best.0 && (k.1 - best.1).abs() < 1e-3
        })
        .collect()
}

/// `font-stretch` (CSS Fonts 4 §5.2 step 4a).
fn stretch_key(d: f32, (lo, hi): (f32, f32)) -> Key {
    if lo <= d && d <= hi {
        (0, 0.0)
    } else if d <= 100.0 {
        if hi < d { (1, d - hi) } else { (2, lo - d) }
    } else if lo > d {
        (1, lo - d)
    } else {
        (2, d - hi)
    }
}

/// Where an oblique search meets the face interval `[lo, hi]` (`None` = never).
/// `stage` identifies the ordered search phase; the returned angle is the first hit.
fn first_hit(lo: f32, hi: f32, from: f32, ascending: bool, bound: Option<f32>) -> Option<f32> {
    // ascending: angles from `from` upwards (bound = exclusive upper end, if any)
    // descending: angles from `from` downwards (bound = exclusive lower end, if any)
    if ascending {
        let a = lo.max(from);
        (a <= hi && bound.is_none_or(|b| a < b)).then_some(a)
    } else {
        let a = hi.min(from);
        (a >= lo && bound.is_none_or(|b| a > b)).then_some(a)
    }
}

/// `font-style` (CSS Fonts 4 §5.2 step 4b). A face is italic (italic value 1), normal (italic
/// 0 and oblique 0deg) or oblique over an angle interval. Returns (stage, distance) and the
/// oblique angle the face is matched at (for `slnt`).
fn slope_match(req: Slope, face: FaceSlope) -> (Key, Option<f32>) {
    const NONE: (Key, Option<f32>) = ((u8::MAX, 0.0), None);
    let (mut lo, mut hi) = match face {
        FaceSlope::Normal => (0.0, 0.0),
        FaceSlope::Oblique(lo, hi) => (lo, hi),
        FaceSlope::Italic => (f32::NAN, f32::NAN),
    };
    let italic = matches!(face, FaceSlope::Italic);
    let normal = matches!(face, FaceSlope::Normal);
    // an oblique hit at stage `k`: distance along the search direction
    let hit = |k: u8, lo: f32, hi: f32, from: f32, asc: bool, bound: Option<f32>, neg: bool| {
        first_hit(lo, hi, from, asc, bound).map(|a| ((k, (a - from).abs()), Some(if neg { -a } else { a })))
    };
    match req {
        Slope::Italic => {
            if italic {
                return ((0, 0.0), None);
            }
            if normal {
                return ((3, 0.0), None); // italic values ≤ 0 (normal faces are italic 0)
            }
            hit(1, lo, hi, 11.0, true, None, false)
                .or_else(|| hit(2, lo, hi, 11.0, false, Some(0.0), false))
                .or_else(|| hit(4, lo, hi, 0.0, false, None, false))
                .unwrap_or(NONE)
        }
        Slope::Oblique(a) => {
            let neg = a < 0.0;
            let a = a.abs();
            if neg {
                (lo, hi) = (-hi, -lo);
            }
            if italic {
                return ((3, 0.0), None);
            }
            if lo <= a && a <= hi {
                return ((0, 0.0), Some(if neg { -a } else { a }));
            }
            let positive = if a >= 11.0 {
                hit(1, lo, hi, a, true, None, neg).or_else(|| hit(2, lo, hi, a, false, Some(0.0), neg))
            } else {
                hit(1, lo, hi, a, false, Some(0.0), neg).or_else(|| hit(2, lo, hi, a, true, None, neg))
            };
            positive
                .or_else(|| hit(4, lo, hi, 0.0, false, None, neg))
                .unwrap_or(NONE)
        }
        Slope::Normal => {
            if italic {
                return ((1, 1.0), None);
            }
            hit(0, lo, hi, 0.0, true, None, false)
                .or_else(|| hit(2, lo, hi, 0.0, false, None, false))
                .unwrap_or(NONE)
        }
    }
}

fn slope_key(req: Slope, face: FaceSlope) -> Key {
    slope_match(req, face).0
}

/// `font-weight` (step 4c), range-aware.
fn weight_key(w: f32, (lo, hi): (f32, f32)) -> Key {
    if lo <= w && w <= hi {
        (0, 0.0)
    } else if (400.0..=500.0).contains(&w) {
        if lo > w && lo <= 500.0 {
            (1, lo - w)
        } else if hi < w {
            (2, w - hi)
        } else {
            (3, lo - w)
        }
    } else if w < 400.0 {
        if hi < w { (1, w - hi) } else { (2, lo - w) }
    } else if lo > w {
        (1, lo - w)
    } else {
        (2, w - hi)
    }
}

fn finite_or(v: f32, d: f32) -> f32 {
    if v.is_finite() { v } else { d }
}

fn range(r: (f32, f32), d: f32, min: f32, max: f32) -> (f32, f32) {
    let a = finite_or(r.0, d).clamp(min, max);
    let b = finite_or(r.1, d).clamp(min, max);
    (a.min(b), a.max(b))
}

impl FaceDescriptors {
    /// Normalised copy: NaN/infinite values replaced by the CSS initial values, reversed
    /// ranges swapped, values clamped to CSS ranges (weight 1–1000, stretch 50–200 %, oblique
    /// −90–90°).
    pub fn normalized(&self) -> FaceDescriptors {
        FaceDescriptors {
            weight: range(self.weight, 400.0, 1.0, 1000.0),
            stretch: range(self.stretch, 100.0, 50.0, 200.0),
            slope: match self.slope {
                FaceSlope::Oblique(lo, hi) => {
                    let (lo, hi) = range((lo, hi), 14.0, -90.0, 90.0);
                    FaceSlope::Oblique(lo, hi)
                }
                s => s,
            },
        }
    }
}

impl Request {
    /// Normalised copy (same rules as [`FaceDescriptors::normalized`]).
    pub fn normalized(&self) -> Request {
        Request {
            weight: finite_or(self.weight, 400.0).clamp(1.0, 1000.0),
            stretch: finite_or(self.stretch, 100.0).clamp(50.0, 200.0),
            slope: match self.slope {
                Slope::Oblique(a) => Slope::Oblique(finite_or(a, 14.0).clamp(-90.0, 90.0)),
                s => s,
            },
        }
    }
}

/// Selects the face set for `req` among `faces` (all of one family). `None` when empty.
/// Inputs are normalised first (see [`FaceDescriptors::normalized`]), so invalid numbers never
/// panic or empty the set.
pub fn select(faces: &[FaceDescriptors], req: Request) -> Option<Selection> {
    if faces.is_empty() {
        return None;
    }
    let faces: Vec<FaceDescriptors> = faces.iter().map(FaceDescriptors::normalized).collect();
    let req = req.normalized();
    let all: Vec<usize> = (0..faces.len()).collect();
    let s = keep_min(all, |i| stretch_key(req.stretch, faces[i].stretch));
    let s = keep_min(s, |i| slope_key(req.slope, faces[i].slope));
    let s = keep_min(s, |i| weight_key(req.weight, faces[i].weight));
    let first = faces[s[0]];
    let weight = req.weight.clamp(first.weight.0, first.weight.1);
    let stretch = req.stretch.clamp(first.stretch.0, first.stretch.1);
    let oblique = slope_match(req.slope, first.slope).1;
    let synthesize_bold = req.weight >= 600.0 && first.weight.1 <= 500.0;
    let synthesize_slant = !matches!(req.slope, Slope::Normal) && matches!(first.slope, FaceSlope::Normal);
    Some(Selection {
        faces: s,
        weight,
        stretch,
        oblique,
        synthesize_bold,
        synthesize_slant,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn face(w: (f32, f32), slope: FaceSlope) -> FaceDescriptors {
        FaceDescriptors {
            weight: w,
            slope,
            stretch: (100.0, 100.0),
        }
    }
    fn req(w: f32, slope: Slope) -> Request {
        Request {
            weight: w,
            slope,
            stretch: 100.0,
        }
    }

    #[test]
    fn regular_and_bold_pick_their_face_without_synthesis() {
        let f = [
            face((400.0, 400.0), FaceSlope::Normal),
            face((700.0, 700.0), FaceSlope::Normal),
        ];
        let s = select(&f, req(700.0, Slope::Normal)).unwrap();
        assert_eq!(s.faces, vec![1]);
        assert!(!s.synthesize_bold);
        let s = select(&f, req(400.0, Slope::Normal)).unwrap();
        assert_eq!(s.faces, vec![0]);
        // 600 > 500: heavier first → bold
        assert_eq!(select(&f, req(600.0, Slope::Normal)).unwrap().faces, vec![1]);
        // 300 < 400: lighter first (none), then heavier ascending → regular
        assert_eq!(select(&f, req(300.0, Slope::Normal)).unwrap().faces, vec![0]);
    }

    #[test]
    fn weight_400_to_500_rule() {
        // desired 450: faces in [450, 500] ascending first, then below descending
        let f = [
            face((300.0, 300.0), FaceSlope::Normal),
            face((500.0, 500.0), FaceSlope::Normal),
            face((600.0, 600.0), FaceSlope::Normal),
        ];
        assert_eq!(select(&f, req(450.0, Slope::Normal)).unwrap().faces, vec![1]);
        let f = [
            face((300.0, 300.0), FaceSlope::Normal),
            face((600.0, 600.0), FaceSlope::Normal),
        ];
        assert_eq!(select(&f, req(450.0, Slope::Normal)).unwrap().faces, vec![0]);
    }

    #[test]
    fn variable_range_matches_exactly_and_static_700_ties() {
        // ADR 0005 §4.1 tests: `font-weight: 100 900` variable face plus a static 700 face.
        let f = [
            face((100.0, 900.0), FaceSlope::Normal),
            face((700.0, 700.0), FaceSlope::Normal),
        ];
        let s = select(&f, req(650.0, Slope::Normal)).unwrap();
        assert_eq!(s.faces, vec![0]);
        assert_eq!(s.weight, 650.0);
        // at 700 both match exactly: both are in the winning set (unicode-range composite);
        // the candidate order (later-declared first) is applied by the store.
        let s = select(&f, req(700.0, Slope::Normal)).unwrap();
        assert_eq!(s.faces, vec![0, 1]);
    }

    #[test]
    fn italic_falls_back_to_oblique_then_normal_with_synthesis() {
        let f = [
            face((400.0, 400.0), FaceSlope::Normal),
            face((400.0, 400.0), FaceSlope::Oblique(10.0, 10.0)),
        ];
        assert_eq!(select(&f, req(400.0, Slope::Italic)).unwrap().faces, vec![1]);
        let f = [face((400.0, 400.0), FaceSlope::Normal)];
        let s = select(&f, req(400.0, Slope::Italic)).unwrap();
        assert!(s.synthesize_slant);
        let s = select(&f, req(700.0, Slope::Normal)).unwrap();
        assert!(s.synthesize_bold);
    }

    #[test]
    fn stretch_narrower_first_below_100() {
        let mut a = face((400.0, 400.0), FaceSlope::Normal);
        a.stretch = (75.0, 75.0);
        let mut b = a;
        b.stretch = (125.0, 125.0);
        let r = Request {
            weight: 400.0,
            slope: Slope::Normal,
            stretch: 87.5,
        };
        assert_eq!(select(&[a, b], r).unwrap().faces, vec![0]);
        let r = Request { stretch: 112.5, ..r };
        assert_eq!(select(&[a, b], r).unwrap().faces, vec![1]);
    }

    #[test]
    fn unicode_range_split_faces_share_the_set() {
        let f = [
            face((400.0, 400.0), FaceSlope::Normal),
            face((400.0, 400.0), FaceSlope::Normal),
            face((700.0, 700.0), FaceSlope::Normal),
        ];
        assert_eq!(select(&f, req(400.0, Slope::Normal)).unwrap().faces, vec![0, 1]);
    }

    fn pick(r: Slope, faces: &[FaceSlope]) -> Vec<usize> {
        let f: Vec<FaceDescriptors> = faces.iter().map(|&s| face((400.0, 400.0), s)).collect();
        select(&f, req(400.0, r)).unwrap().faces
    }

    #[test]
    fn oblique_ordering_table_css_fonts_4() {
        use FaceSlope::{Italic as I, Normal as N, Oblique as O};
        let cases: &[(Slope, &[FaceSlope], usize)] = &[
            // below 11°: lower angles first (descending to 0), then higher
            (Slope::Oblique(5.0), &[O(4.0, 4.0), O(6.0, 6.0)], 0),
            (Slope::Oblique(5.0), &[O(6.0, 6.0), O(30.0, 30.0)], 0),
            (Slope::Oblique(5.0), &[N, O(6.0, 6.0)], 1),
            // ≥ 11°: higher first (ascending), then lower (descending)
            (Slope::Oblique(14.0), &[O(12.0, 12.0), O(16.0, 16.0)], 1),
            (Slope::Oblique(14.0), &[O(5.0, 5.0), O(12.0, 12.0)], 1),
            // range containing the request wins
            (Slope::Oblique(14.0), &[O(0.0, 20.0), O(14.5, 14.5)], 0),
            // positive obliques exhausted → italic → oblique ≤ 0 (normal)
            (Slope::Oblique(14.0), &[N, I], 1),
            (Slope::Oblique(14.0), &[N, O(-10.0, -10.0)], 0),
            // negative angles mirror the search
            (Slope::Oblique(-14.0), &[O(-12.0, -12.0), O(-16.0, -16.0)], 1),
            (Slope::Oblique(-5.0), &[O(-4.0, -4.0), O(-6.0, -6.0)], 0),
            (Slope::Oblique(-14.0), &[O(14.0, 14.0), N], 1),
            // italic: italic, then oblique ≥ 11 ascending, then below 11 descending, then normal
            (Slope::Italic, &[N, O(12.0, 12.0)], 1),
            (Slope::Italic, &[O(8.0, 8.0), O(20.0, 20.0)], 1),
            (Slope::Italic, &[O(4.0, 4.0), O(8.0, 8.0)], 1),
            (Slope::Italic, &[N, O(-12.0, -12.0)], 0),
            // normal: normal (oblique 0) / oblique ≥ 0 ascending, then italic, then negative
            (Slope::Normal, &[O(5.0, 5.0), I], 0),
            (Slope::Normal, &[I, O(-5.0, -5.0)], 0),
        ];
        for (i, (r, faces, want)) in cases.iter().enumerate() {
            assert_eq!(pick(*r, faces), vec![*want], "case {i}: {r:?} {faces:?}");
        }
    }

    #[test]
    fn resolved_values_and_invalid_inputs() {
        let f = [FaceDescriptors {
            weight: (900.0, 100.0), // reversed: normalised
            slope: FaceSlope::Oblique(0.0, 20.0),
            stretch: (75.0, 125.0),
        }];
        let s = select(
            &f,
            Request {
                weight: 650.0,
                slope: Slope::Oblique(15.0),
                stretch: 87.5,
            },
        )
        .unwrap();
        assert_eq!((s.weight, s.stretch, s.oblique), (650.0, 87.5, Some(15.0)));
        let s = select(
            &f,
            Request {
                weight: f32::NAN,
                slope: Slope::Oblique(f32::INFINITY),
                stretch: f32::NAN,
            },
        )
        .unwrap();
        assert_eq!(s.faces, vec![0]);
        assert_eq!(s.weight, 400.0);
        let nan = [FaceDescriptors {
            weight: (f32::NAN, f32::NAN),
            slope: FaceSlope::Normal,
            stretch: (f32::NAN, 1e9),
        }];
        assert!(select(&nan, req(700.0, Slope::Normal)).is_some());
    }
}
