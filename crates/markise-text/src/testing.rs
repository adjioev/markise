//! Synthetic test fonts (ADR 0005 T1: "synthetic fonts generated in tests").
//!
//! A tiny, dependency-free sfnt writer: every mapped character gets a rectangular TrueType glyph
//! whose advance is chosen by the test, so tests can assert which face rendered a character from
//! glyph ids and advances alone. Supports a name table (family, full and PostScript names), OS/2
//! weight/italic, cmap format 12 plus format 14 (variation sequences), and a GSUB `liga` lookup.
//! Also writes WOFF 1.0 and (null-transform) WOFF2 wrappers, so the decoder path is testable
//! without committing font files (R16: no bundled fonts, also for fixtures).
//!
//! Not production code; public (hidden) so integration tests, benches and spikes can use it.

use std::collections::BTreeMap;

/// Per variation selector: default-UVS bases and non-default (base, glyph) mappings.
type UvsLists = (Vec<u32>, Vec<(u32, u32)>);

/// One synthetic face.
#[derive(Clone, Debug)]
pub struct TestFont {
    pub family: String,
    pub subfamily: String,
    pub full_name: String,
    pub postscript_name: String,
    pub weight: u16,
    pub italic: bool,
    pub units_per_em: u16,
    pub ascent: i16,
    pub descent: i16,
    /// (character, advance in font units); advance 0 makes a zero-width (mark-like) glyph.
    pub chars: Vec<(char, u16)>,
    /// Non-default variation sequences (base, selector, advance): each gets its own glyph.
    pub variation_sequences: Vec<(char, char, u16)>,
    /// Default variation sequences (cmap-14 default UVS: base glyph used as is).
    pub default_sequences: Vec<(char, char)>,
    /// Non-default sequences mapped to glyph 0 (malformed/unsupported marker).
    pub null_sequences: Vec<(char, char)>,
    /// Ligatures: component characters (all must be in `chars`) → one glyph with this advance.
    pub ligatures: Vec<(Vec<char>, u16)>,
    /// Adds an (empty) COLR v0 + CPAL table pair so the face counts as a colour face.
    pub color: bool,
    /// Raw tables added (or replacing generated ones) at build time.
    pub extra_tables: Vec<([u8; 4], Vec<u8>)>,
}

impl TestFont {
    /// A regular face named `family` with no characters yet.
    pub fn new(family: &str) -> Self {
        let ps = family.replace(' ', "");
        TestFont {
            family: family.to_owned(),
            subfamily: "Regular".into(),
            full_name: format!("{family} Regular"),
            postscript_name: format!("{ps}-Regular"),
            weight: 400,
            italic: false,
            units_per_em: 1000,
            ascent: 800,
            descent: -200,
            chars: Vec::new(),
            variation_sequences: Vec::new(),
            default_sequences: Vec::new(),
            null_sequences: Vec::new(),
            ligatures: Vec::new(),
            color: false,
            extra_tables: Vec::new(),
        }
    }

    pub fn with_table(mut self, tag: &[u8; 4], data: Vec<u8>) -> Self {
        self.extra_tables.push((*tag, data));
        self
    }

    pub fn colored(mut self) -> Self {
        self.color = true;
        self
    }

    /// Sets weight/italic and derives subfamily, full and PostScript names.
    pub fn style(mut self, weight: u16, italic: bool) -> Self {
        self.weight = weight;
        self.italic = italic;
        let w = match weight {
            700 => "Bold",
            400 => "Regular",
            _ => "W",
        };
        let sub = match (w, italic) {
            ("Regular", true) => "Italic".to_string(),
            ("W", i) => format!("W{weight}{}", if i { " Italic" } else { "" }),
            (w, true) => format!("{w} Italic"),
            (w, false) => w.to_string(),
        };
        let ps = self.family.replace(' ', "");
        self.full_name = format!("{} {sub}", self.family);
        self.postscript_name = format!("{ps}-{}", sub.replace(' ', ""));
        self.subfamily = sub;
        self
    }

    /// Maps every character of `s` with the given advance.
    pub fn with_chars(mut self, s: &str, advance: u16) -> Self {
        for c in s.chars() {
            if !self.chars.iter().any(|(x, _)| *x == c) {
                self.chars.push((c, advance));
            }
        }
        self
    }

    /// Maps every character in `lo..=hi` with the given advance.
    pub fn with_range(mut self, lo: char, hi: char, advance: u16) -> Self {
        for c in lo..=hi {
            if !self.chars.iter().any(|(x, _)| *x == c) {
                self.chars.push((c, advance));
            }
        }
        self
    }

    pub fn with_variation_sequence(mut self, base: char, selector: char, advance: u16) -> Self {
        self.variation_sequences.push((base, selector, advance));
        self
    }

    pub fn with_default_sequence(mut self, base: char, selector: char) -> Self {
        self.default_sequences.push((base, selector));
        self
    }

    pub fn with_null_sequence(mut self, base: char, selector: char) -> Self {
        self.null_sequences.push((base, selector));
        self
    }

    pub fn with_ligature(mut self, components: &str, advance: u16) -> Self {
        self.ligatures.push((components.chars().collect(), advance));
        self
    }

    /// Glyph id of `c` in the built font (0 if unmapped). Glyph order: .notdef, `chars` in the
    /// order given, then variation sequences, then ligatures.
    pub fn glyph_of(&self, c: char) -> u32 {
        self.chars.iter().position(|(x, _)| *x == c).map_or(0, |i| i as u32 + 1)
    }

    pub fn glyph_of_sequence(&self, base: char, selector: char) -> u32 {
        self.variation_sequences
            .iter()
            .position(|(b, s, _)| *b == base && *s == selector)
            .map_or(0, |i| (1 + self.chars.len() + i) as u32)
    }

    pub fn glyph_of_ligature(&self, components: &str) -> u32 {
        let v: Vec<char> = components.chars().collect();
        self.ligatures.iter().position(|(c, _)| *c == v).map_or(0, |i| {
            (1 + self.chars.len() + self.variation_sequences.len() + i) as u32
        })
    }

    /// Serialises the face as a TrueType sfnt.
    pub fn build(&self) -> Vec<u8> {
        let mut advances: Vec<u16> = vec![500];
        advances.extend(self.chars.iter().map(|(_, a)| *a));
        advances.extend(self.variation_sequences.iter().map(|(_, _, a)| *a));
        advances.extend(self.ligatures.iter().map(|(_, a)| *a));
        let n = advances.len();

        // glyf/loca: rectangle per glyph (advance 0 → small box left of the origin, like a mark)
        let mut glyf = Vec::new();
        let mut loca: Vec<u32> = Vec::with_capacity(n + 1);
        for &adv in &advances {
            loca.push(glyf.len() as u32);
            let (x0, x1) = if adv == 0 {
                (-200i16, -50i16)
            } else {
                (50, (adv as i16 - 50).max(60))
            };
            let (y0, y1) = (0i16, 700i16);
            be16(&mut glyf, 1); // contours
            for v in [x0, y0, x1, y1] {
                be16(&mut glyf, v as u16);
            }
            be16(&mut glyf, 3); // endPts
            be16(&mut glyf, 0); // instructions
            glyf.extend([1u8; 4]); // on-curve, 16-bit coords
            let xs = [x0, x1, x1, x0];
            let ys = [y0, y0, y1, y1];
            let mut px = 0i16;
            for x in xs {
                be16(&mut glyf, (x - px) as u16);
                px = x;
            }
            let mut py = 0i16;
            for y in ys {
                be16(&mut glyf, (y - py) as u16);
                py = y;
            }
            while glyf.len() % 4 != 0 {
                glyf.push(0);
            }
        }
        loca.push(glyf.len() as u32);
        let mut loca_b = Vec::new();
        for o in &loca {
            be32(&mut loca_b, *o);
        }

        let mut hmtx = Vec::new();
        for &a in &advances {
            be16(&mut hmtx, a);
            be16(&mut hmtx, 50);
        }

        let mut head = Vec::new();
        be32(&mut head, 0x0001_0000);
        be32(&mut head, 0x0001_0000);
        be32(&mut head, 0); // checksum adjustment
        be32(&mut head, 0x5F0F_3CF5);
        be16(&mut head, 0b11); // baseline at y=0, lsb at x=0
        be16(&mut head, self.units_per_em);
        head.extend([0u8; 16]); // created, modified
        for v in [-200i16, self.descent, 2000, self.ascent] {
            be16(&mut head, v as u16);
        }
        let mac_style = u16::from(self.weight >= 700) | (u16::from(self.italic) << 1);
        be16(&mut head, mac_style);
        be16(&mut head, 8);
        be16(&mut head, 2);
        be16(&mut head, 1); // long loca
        be16(&mut head, 0);

        let mut hhea = Vec::new();
        be32(&mut hhea, 0x0001_0000);
        be16(&mut hhea, self.ascent as u16);
        be16(&mut hhea, self.descent as u16);
        be16(&mut hhea, 0);
        be16(&mut hhea, *advances.iter().max().unwrap_or(&0));
        be16(&mut hhea, (-200i16) as u16);
        be16(&mut hhea, 0);
        be16(&mut hhea, 2000);
        be16(&mut hhea, 1);
        be16(&mut hhea, 0);
        be16(&mut hhea, 0);
        hhea.extend([0u8; 8]);
        be16(&mut hhea, 0);
        be16(&mut hhea, n as u16);

        let mut maxp = Vec::new();
        be32(&mut maxp, 0x0001_0000);
        be16(&mut maxp, n as u16);
        for v in [4u16, 1, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0] {
            be16(&mut maxp, v);
        }

        let mut post = Vec::new();
        be32(&mut post, 0x0003_0000);
        be32(&mut post, if self.italic { (-12i32 << 16) as u32 } else { 0 });
        be16(&mut post, (-100i16) as u16);
        be16(&mut post, 50);
        post.extend([0u8; 20]);

        let mut os2 = Vec::new();
        be16(&mut os2, 4);
        be16(&mut os2, 500);
        be16(&mut os2, self.weight);
        be16(&mut os2, 5); // normal width
        be16(&mut os2, 0);
        for v in [650i16, 600, 0, 75, 650, 600, 0, 350, 50, 250] {
            be16(&mut os2, v as u16);
        }
        be16(&mut os2, 0);
        os2.extend([0u8; 10]);
        for _ in 0..4 {
            be32(&mut os2, 0);
        }
        os2.extend(*b"MKTS");
        let fs_sel = if self.italic { 1 } else { 0 }
            | if self.weight >= 700 { 1 << 5 } else { 0 }
            | if !self.italic && self.weight < 700 { 1 << 6 } else { 0 }
            | 1 << 7; // use typo metrics
        be16(&mut os2, fs_sel);
        let first = self.chars.iter().map(|(c, _)| *c as u32).min().unwrap_or(0x20);
        let last = self.chars.iter().map(|(c, _)| *c as u32).max().unwrap_or(0x20);
        be16(&mut os2, first.min(0xFFFF) as u16);
        be16(&mut os2, last.min(0xFFFF) as u16);
        be16(&mut os2, self.ascent as u16);
        be16(&mut os2, self.descent as u16);
        be16(&mut os2, 0);
        be16(&mut os2, self.ascent as u16);
        be16(&mut os2, (-self.descent) as u16);
        be32(&mut os2, 1);
        be32(&mut os2, 0);
        for v in [500u16, 700, 0, 0x20, 2] {
            be16(&mut os2, v);
        }

        let mut tables: BTreeMap<[u8; 4], Vec<u8>> = BTreeMap::new();
        tables.insert(*b"cmap", self.cmap());
        tables.insert(*b"glyf", glyf);
        tables.insert(*b"head", head);
        tables.insert(*b"hhea", hhea);
        tables.insert(*b"hmtx", hmtx);
        tables.insert(*b"loca", loca_b);
        tables.insert(*b"maxp", maxp);
        tables.insert(*b"name", self.name());
        tables.insert(*b"OS/2", os2);
        tables.insert(*b"post", post);
        if !self.ligatures.is_empty() {
            tables.insert(*b"GSUB", self.gsub());
        }
        if self.color {
            // COLR v0 with no records; CPAL v0 with one palette of one entry
            tables.insert(*b"COLR", vec![0, 0, 0, 0, 0, 0, 0, 14, 0, 0, 0, 14, 0, 0]);
            let mut cpal = Vec::new();
            for v in [0u16, 1, 1, 1] {
                be16(&mut cpal, v);
            }
            be32(&mut cpal, 14);
            be16(&mut cpal, 0);
            cpal.extend([0u8, 0, 255, 255]);
            tables.insert(*b"CPAL", cpal);
        }
        for (t, d) in &self.extra_tables {
            tables.insert(*t, d.clone());
        }
        sfnt(&tables)
    }

    fn cmap(&self) -> Vec<u8> {
        // format 12
        let mut map: Vec<(u32, u32)> = self
            .chars
            .iter()
            .enumerate()
            .map(|(i, (c, _))| (*c as u32, i as u32 + 1))
            .collect();
        map.sort_unstable();
        let mut groups: Vec<(u32, u32, u32)> = Vec::new();
        for (c, g) in map {
            if let Some(last) = groups.last_mut()
                && last.1 + 1 == c
                && last.2 + (c - last.0) == g
            {
                last.1 = c;
                continue;
            }
            groups.push((c, c, g));
        }
        let mut f12 = Vec::new();
        be16(&mut f12, 12);
        be16(&mut f12, 0);
        be32(&mut f12, 16 + 12 * groups.len() as u32);
        be32(&mut f12, 0);
        be32(&mut f12, groups.len() as u32);
        for (s, e, g) in &groups {
            be32(&mut f12, *s);
            be32(&mut f12, *e);
            be32(&mut f12, *g);
        }
        // format 14: per selector a default-UVS range list and a non-default mapping list
        let mut f14 = Vec::new();
        let mut by_sel: BTreeMap<u32, UvsLists> = BTreeMap::new();
        for (b, s, _) in &self.variation_sequences {
            by_sel
                .entry(*s as u32)
                .or_default()
                .1
                .push((*b as u32, self.glyph_of_sequence(*b, *s)));
        }
        for (b, s) in &self.null_sequences {
            by_sel.entry(*s as u32).or_default().1.push((*b as u32, 0));
        }
        for (b, s) in &self.default_sequences {
            by_sel.entry(*s as u32).or_default().0.push(*b as u32);
        }
        if !by_sel.is_empty() {
            let header = 10 + 11 * by_sel.len();
            let mut body = Vec::new();
            let mut records = Vec::new();
            for (sel, (mut defs, mut maps)) in by_sel {
                let def_off = if defs.is_empty() {
                    0
                } else {
                    defs.sort_unstable();
                    let off = header + body.len();
                    be32(&mut body, defs.len() as u32);
                    for b in defs {
                        be24(&mut body, b);
                        body.push(0); // additionalCount
                    }
                    off
                };
                let map_off = if maps.is_empty() {
                    0
                } else {
                    maps.sort_unstable();
                    let off = header + body.len();
                    be32(&mut body, maps.len() as u32);
                    for (b, g) in maps {
                        be24(&mut body, b);
                        be16(&mut body, g as u16);
                    }
                    off
                };
                records.push((sel, def_off as u32, map_off as u32));
            }
            be16(&mut f14, 14);
            be32(&mut f14, (header + body.len()) as u32);
            be32(&mut f14, records.len() as u32);
            for (sel, d, m) in records {
                be24(&mut f14, sel);
                be32(&mut f14, d);
                be32(&mut f14, m);
            }
            f14.extend(body);
        }
        let mut out = Vec::new();
        let n = if f14.is_empty() { 2u16 } else { 3 };
        be16(&mut out, 0);
        be16(&mut out, n);
        let base = 4 + 8 * n as u32;
        // records sorted by (platform, encoding): (0,4) f12, (0,5) f14, (3,10) f12
        be16(&mut out, 0);
        be16(&mut out, 4);
        be32(&mut out, base);
        if !f14.is_empty() {
            be16(&mut out, 0);
            be16(&mut out, 5);
            be32(&mut out, base + f12.len() as u32);
        }
        be16(&mut out, 3);
        be16(&mut out, 10);
        be32(&mut out, base);
        out.extend(&f12);
        out.extend(&f14);
        out
    }

    fn name(&self) -> Vec<u8> {
        let entries: [(u16, &str); 5] = [
            (1, &self.family),
            (2, &self.subfamily),
            (3, &self.postscript_name),
            (4, &self.full_name),
            (6, &self.postscript_name),
        ];
        let mut storage = Vec::new();
        let mut recs = Vec::new();
        for (id, s) in entries {
            let off = storage.len();
            for u in s.encode_utf16() {
                be16(&mut storage, u);
            }
            recs.push((id, off as u16, (storage.len() - off) as u16));
        }
        let mut out = Vec::new();
        be16(&mut out, 0);
        be16(&mut out, recs.len() as u16);
        be16(&mut out, (6 + 12 * recs.len()) as u16);
        for (id, off, len) in recs {
            for v in [3u16, 1, 0x409, id, len, off] {
                be16(&mut out, v);
            }
        }
        out.extend(storage);
        out
    }

    fn gsub(&self) -> Vec<u8> {
        // GSUB 1.0: ScriptList (DFLT + latn → feature 0), FeatureList (liga → lookup 0),
        // LookupList (one type-4 lookup, one LigatureSubstFormat1 subtable).
        let mut by_first: BTreeMap<u32, Vec<(Vec<u32>, u32)>> = BTreeMap::new();
        for (comps, _) in &self.ligatures {
            let s: String = comps.iter().collect();
            let lig = self.glyph_of_ligature(&s);
            let gids: Vec<u32> = comps.iter().map(|c| self.glyph_of(*c)).collect();
            by_first.entry(gids[0]).or_default().push((gids[1..].to_vec(), lig));
        }
        // LigatureSubst subtable
        let firsts: Vec<u32> = by_first.keys().copied().collect();
        let n_sets = firsts.len();
        let mut sub = Vec::new();
        let sets_start = 6 + 2 * n_sets;
        // build sets after header; coverage at the end
        let mut sets = Vec::new();
        let mut set_offsets = Vec::new();
        for f in &firsts {
            set_offsets.push(sets_start + sets.len());
            let ligs = &by_first[f];
            let mut set = Vec::new();
            be16(&mut set, ligs.len() as u16);
            let mut lig_tables = Vec::new();
            let mut lig_offs = Vec::new();
            for (rest, lig) in ligs {
                lig_offs.push(2 + 2 * ligs.len() + lig_tables.len());
                be16(&mut lig_tables, *lig as u16);
                be16(&mut lig_tables, rest.len() as u16 + 1);
                for g in rest {
                    be16(&mut lig_tables, *g as u16);
                }
            }
            for o in lig_offs {
                be16(&mut set, o as u16);
            }
            set.extend(lig_tables);
            sets.extend(set);
        }
        let cov_off = sets_start + sets.len();
        be16(&mut sub, 1);
        be16(&mut sub, cov_off as u16);
        be16(&mut sub, n_sets as u16);
        for o in set_offsets {
            be16(&mut sub, o as u16);
        }
        sub.extend(sets);
        be16(&mut sub, 1); // coverage format 1
        be16(&mut sub, n_sets as u16);
        for f in &firsts {
            be16(&mut sub, *f as u16);
        }
        // Lookup
        let mut lookup = Vec::new();
        be16(&mut lookup, 4);
        be16(&mut lookup, 0);
        be16(&mut lookup, 1);
        be16(&mut lookup, 8);
        lookup.extend(sub);
        let mut lookup_list = Vec::new();
        be16(&mut lookup_list, 1);
        be16(&mut lookup_list, 4);
        lookup_list.extend(lookup);
        // FeatureList
        let mut feature_list = Vec::new();
        be16(&mut feature_list, 1);
        feature_list.extend(*b"liga");
        be16(&mut feature_list, 8);
        be16(&mut feature_list, 0); // featureParams
        be16(&mut feature_list, 1);
        be16(&mut feature_list, 0);
        // ScriptList: two scripts sharing one Script table layout
        let script = {
            let mut s = Vec::new();
            be16(&mut s, 4); // default LangSys offset
            be16(&mut s, 0); // langSysCount
            be16(&mut s, 0); // lookupOrder
            be16(&mut s, 0xFFFF); // reqFeatureIndex
            be16(&mut s, 1);
            be16(&mut s, 0);
            s
        };
        let mut script_list = Vec::new();
        be16(&mut script_list, 2);
        let s1 = 2 + 6 * 2;
        let s2 = s1 + script.len();
        script_list.extend(*b"DFLT");
        be16(&mut script_list, s1 as u16);
        script_list.extend(*b"latn");
        be16(&mut script_list, s2 as u16);
        script_list.extend(&script);
        script_list.extend(&script);

        let mut out = Vec::new();
        be32(&mut out, 0x0001_0000);
        let sl = 10;
        let fl = sl + script_list.len();
        let ll = fl + feature_list.len();
        be16(&mut out, sl as u16);
        be16(&mut out, fl as u16);
        be16(&mut out, ll as u16);
        out.extend(script_list);
        out.extend(feature_list);
        out.extend(lookup_list);
        out
    }
}

fn be16(v: &mut Vec<u8>, x: u16) {
    v.extend(x.to_be_bytes());
}
fn be24(v: &mut Vec<u8>, x: u32) {
    v.extend(&x.to_be_bytes()[1..]);
}
fn be32(v: &mut Vec<u8>, x: u32) {
    v.extend(x.to_be_bytes());
}

pub use crate::face::sfnt;

/// Parses an sfnt's table directory: (tag, checksum, data).
fn tables_of(sfnt: &[u8]) -> Vec<([u8; 4], u32, Vec<u8>)> {
    let n = u16::from_be_bytes([sfnt[4], sfnt[5]]) as usize;
    (0..n)
        .map(|i| {
            let r = &sfnt[12 + 16 * i..28 + 16 * i];
            let tag = [r[0], r[1], r[2], r[3]];
            let ck = u32::from_be_bytes([r[4], r[5], r[6], r[7]]);
            let off = u32::from_be_bytes([r[8], r[9], r[10], r[11]]) as usize;
            let len = u32::from_be_bytes([r[12], r[13], r[14], r[15]]) as usize;
            (tag, ck, sfnt[off..off + len].to_vec())
        })
        .collect()
}

/// Wraps an sfnt as WOFF 1.0 with the given per-table compressor (e.g. zlib). The compressor may
/// return the input unchanged; tables are stored compressed only when that is smaller.
pub fn woff1(sfnt_bytes: &[u8], compress: impl Fn(&[u8]) -> Vec<u8>) -> Vec<u8> {
    let tables = tables_of(sfnt_bytes);
    let n = tables.len();
    let mut dir = Vec::new();
    let mut data = Vec::new();
    let data_start = 44 + 20 * n;
    let mut total_sfnt = 12 + 16 * n as u32;
    for (tag, ck, t) in &tables {
        let c = compress(t);
        let stored = if c.len() < t.len() { c } else { t.clone() };
        let off = data_start + data.len();
        dir.extend(tag);
        be32(&mut dir, off as u32);
        be32(&mut dir, stored.len() as u32);
        be32(&mut dir, t.len() as u32);
        be32(&mut dir, *ck);
        data.extend(&stored);
        while data.len() % 4 != 0 {
            data.push(0);
        }
        total_sfnt += (t.len() as u32 + 3) & !3;
    }
    let mut out = Vec::new();
    out.extend(*b"wOFF");
    be32(&mut out, 0x0001_0000);
    be32(&mut out, (data_start + data.len()) as u32);
    be16(&mut out, n as u16);
    be16(&mut out, 0);
    be32(&mut out, total_sfnt);
    be16(&mut out, 1);
    be16(&mut out, 0);
    out.extend([0u8; 20]);
    out.extend(dir);
    out.extend(data);
    out
}

const WOFF2_KNOWN: [&[u8; 4]; 13] = [
    b"cmap", b"head", b"hhea", b"hmtx", b"maxp", b"name", b"OS/2", b"post", b"cvt ", b"fpgm", b"glyf", b"loca", b"prep",
];

fn base128(v: &mut Vec<u8>, mut x: u32) {
    let mut bytes = vec![(x & 0x7F) as u8];
    x >>= 7;
    while x > 0 {
        bytes.push(0x80 | (x & 0x7F) as u8);
        x >>= 7;
    }
    bytes.reverse();
    v.extend(bytes);
}

/// Wraps an sfnt as WOFF2 using the null transform for every table (glyf/loca transform
/// version 3), compressing the table stream with `brotli`.
pub fn woff2(sfnt_bytes: &[u8], brotli: impl Fn(&[u8]) -> Vec<u8>) -> Vec<u8> {
    let mut tables = tables_of(sfnt_bytes);
    // keep loca right after glyf (decoders reconstruct them together)
    tables.sort_by_key(|(t, _, _)| match t {
        b"loca" => (*b"glyg", 1),
        _ => (*t, 0),
    });
    let mut dir = Vec::new();
    let mut stream = Vec::new();
    let mut total_sfnt = 12 + 16 * tables.len() as u32;
    for (tag, _, t) in &tables {
        let known = WOFF2_KNOWN.iter().position(|k| *k == tag);
        let xform: u8 = if tag == b"glyf" || tag == b"loca" { 3 } else { 0 };
        match known {
            Some(i) => dir.push((xform << 6) | i as u8),
            None => {
                dir.push((xform << 6) | 63);
                dir.extend(tag);
            }
        }
        base128(&mut dir, t.len() as u32);
        stream.extend(t);
        total_sfnt += (t.len() as u32 + 3) & !3;
    }
    let compressed = brotli(&stream);
    let mut body = dir;
    let header_len = 48;
    let comp_off = header_len + body.len();
    body.extend(&compressed);
    while (header_len + body.len()) % 4 != 0 {
        body.push(0);
    }
    let mut out = Vec::new();
    out.extend(*b"wOF2");
    be32(&mut out, 0x0001_0000);
    be32(&mut out, (header_len + body.len()) as u32);
    be16(&mut out, tables.len() as u16);
    be16(&mut out, 0);
    be32(&mut out, total_sfnt);
    be32(&mut out, compressed.len() as u32);
    be16(&mut out, 1);
    be16(&mut out, 0);
    out.extend([0u8; 20]);
    debug_assert_eq!(out.len(), header_len);
    let _ = comp_off;
    out.extend(body);
    out
}
