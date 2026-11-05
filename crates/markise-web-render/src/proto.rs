//! The frames between the client and the helper (ADR 0023). Both sides treat what they read as
//! untrusted: a page's code runs behind the helper. A frame is `u32 LE` + JSON, `u32 LE` + bytes;
//! a length over its bound or JSON of the wrong shape ends the conversation.

use std::io::{self, Read, Write};

use serde_json::{Map, Value, json};

/// Bumped on any change a peer would misread; the first frame of a helper says which it speaks.
pub const PROTOCOL: u32 = 1;
/// The scheme a page's files come from. Nothing else loads.
pub const SCHEME: &str = "markise-block";
/// The host of the files of a package.
pub const PACKAGE_HOST: &str = "package";
/// The host of the block's images, in its document's folder.
pub const DOCUMENT_HOST: &str = "document";
/// The page itself, answered by the helper: no file can stand in for it.
pub const SHELL_PATH: &str = "/__markise_shell__.html";

/// The JSON part of a frame.
pub const MAX_JSON: usize = 4 << 20;
/// The bytes of a file a page gets: the package format's bound on one file (ADR 0019).
pub const MAX_FILE: usize = 32 << 20;
/// A block's HTML.
pub const MAX_HTML: usize = 1 << 20;
/// A path of a file.
pub const MAX_PATH: usize = 1024;
/// Style sheets a block takes.
pub const MAX_STYLES: usize = 16;
/// Variables of the theme a page gets.
pub const MAX_VARS: usize = 64;
/// One side of an image, in pixels: what a GPU texture can surely hold.
pub const MAX_SIDE_PX: u32 = 8192;
/// All of an image, in pixels (RGBA: four bytes each).
pub const MAX_PIXELS: u64 = 24_000_000;
/// The width of a page, in CSS pixels.
pub const WIDTHS: std::ops::RangeInclusive<u32> = 16..=4096;
/// The scale of an image over CSS pixels.
pub const SCALES: std::ops::RangeInclusive<f64> = 0.5..=4.0;
/// The time a render may take, in milliseconds.
pub const LIMITS_MS: std::ops::RangeInclusive<u32> = 100..=30_000;

fn bad(what: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, what.into())
}

/// `n` bytes, grown as they come, so a false length costs nothing before the bytes are there.
fn read_bytes(r: &mut impl Read, n: usize) -> io::Result<Vec<u8>> {
    let mut buf = Vec::new();
    r.take(n as u64).read_to_end(&mut buf)?;
    if buf.len() != n {
        return Err(io::ErrorKind::UnexpectedEof.into());
    }
    Ok(buf)
}

fn read_part(r: &mut impl Read, max: usize) -> io::Result<Vec<u8>> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len)?;
    let n = u32::from_le_bytes(len) as usize;
    if n > max {
        return Err(bad(format!("a frame part of {n} bytes")));
    }
    read_bytes(r, n)
}

/// How many bytes a message may come with.
enum Attachment {
    Exactly(usize),
    AtMost(usize),
}

/// One frame. Its JSON is read and judged first: only then is the length of its bytes looked at, and
/// only a length its message allows is read. So what is wrong costs no more than its JSON.
fn read_frame<M>(
    r: &mut impl Read,
    decode: impl FnOnce(&Value) -> io::Result<(M, Attachment)>,
) -> io::Result<(M, Vec<u8>)> {
    let json = read_part(r, MAX_JSON)?;
    let value: Value = serde_json::from_slice(&json).map_err(|e| bad(e.to_string()))?;
    let (message, attachment) = decode(&value)?;
    let mut len = [0u8; 4];
    r.read_exact(&mut len)?;
    let n = u32::from_le_bytes(len) as usize;
    let fine = match attachment {
        Attachment::Exactly(m) => n == m,
        Attachment::AtMost(m) => n <= m,
    };
    if !fine {
        return Err(bad(format!("an attachment of {n} bytes")));
    }
    Ok((message, read_bytes(r, n)?))
}

/// The next message for a helper, with its bytes.
pub fn read_to_helper(r: &mut impl Read) -> io::Result<(ToHelper, Vec<u8>)> {
    read_frame(r, |v| {
        let message = ToHelper::decode(v)?;
        let attachment = match message {
            ToHelper::File { found: true, .. } => Attachment::AtMost(MAX_FILE),
            _ => Attachment::Exactly(0),
        };
        Ok((message, attachment))
    })
}

/// The next message from a helper, with its bytes.
pub fn read_from_helper(r: &mut impl Read) -> io::Result<(FromHelper, Vec<u8>)> {
    read_frame(r, |v| {
        let message = FromHelper::decode(v)?;
        let attachment = match &message {
            FromHelper::Rendered(i) => Attachment::Exactly(i.px_width as usize * i.px_height as usize * 4),
            _ => Attachment::Exactly(0),
        };
        Ok((message, attachment))
    })
}

pub fn write_frame(w: &mut impl Write, json: &Value, bytes: &[u8]) -> io::Result<()> {
    let text = serde_json::to_vec(json)?;
    if text.len() > MAX_JSON || bytes.len() > (MAX_PIXELS as usize * 4).max(MAX_FILE) {
        return Err(bad("a frame too large to send"));
    }
    w.write_all(&(text.len() as u32).to_le_bytes())?;
    w.write_all(&text)?;
    w.write_all(&(bytes.len() as u32).to_le_bytes())?;
    w.write_all(bytes)?;
    w.flush()
}

/// Whose file a page asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Area {
    Package,
    Document,
}

impl Area {
    pub fn host(self) -> &'static str {
        match self {
            Area::Package => PACKAGE_HOST,
            Area::Document => DOCUMENT_HOST,
        }
    }

    fn from_host(host: &str) -> Option<Area> {
        match host {
            PACKAGE_HOST => Some(Area::Package),
            DOCUMENT_HOST => Some(Area::Document),
            _ => None,
        }
    }
}

/// A file of an area as a page's URL: its path segments percent-encoded, slashes kept.
pub fn file_url(area: Area, path: &str) -> String {
    let mut out = format!("{SCHEME}://{}/", area.host());
    for b in path.bytes() {
        match b {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// What the helper's scheme handler makes of a URL: the area and the decoded relative path, or
/// `None` for anything that is not a file of an area. Query and fragment do not name a file. The
/// path is still untrusted: the client validates it against the area it belongs to.
pub fn parse_file_url(url: &str) -> Option<(Area, String)> {
    let rest = url.strip_prefix(SCHEME)?.strip_prefix("://")?;
    let (host, path) = rest.split_once('/')?;
    let area = Area::from_host(host)?;
    let path = path.split(['?', '#']).next()?;
    let mut bytes = Vec::with_capacity(path.len());
    let mut it = path.bytes();
    while let Some(b) = it.next() {
        if b == b'%' {
            let hi = (it.next()? as char).to_digit(16)?;
            let lo = (it.next()? as char).to_digit(16)?;
            bytes.push((hi * 16 + lo) as u8);
        } else {
            bytes.push(b);
        }
    }
    let path = String::from_utf8(bytes).ok()?;
    (!path.is_empty() && path.len() <= MAX_PATH).then_some((area, path))
}

/// The image of one block: all the helper needs. `html` and every other value reach the page as
/// data, never as script text.
#[derive(Clone, Debug, PartialEq)]
pub struct RenderRequest {
    pub id: u64,
    /// One web data store and one web view per session: nothing is shared between two.
    pub session: String,
    /// The page's width in CSS pixels.
    pub width: u32,
    /// Image pixels per CSS pixel.
    pub scale: f64,
    pub dark: bool,
    /// `<html lang>`.
    pub lang: String,
    /// CSS variables on `<html>`, by name.
    pub vars: Vec<(String, String)>,
    /// The markup put into the page.
    pub html: String,
    /// Style sheets in order, by path in the package.
    pub styles: Vec<String>,
    /// An ES module in the package whose default export gets the markup's first element.
    pub module: Option<String>,
    /// The helper gives up after this long, and exits: the page may be spinning.
    pub limit_ms: u32,
}

/// From the client to the helper.
#[derive(Clone, Debug, PartialEq)]
pub enum ToHelper {
    Render(RenderRequest),
    /// Do not spend more on this render; a [`FromHelper::Failed`] answers it.
    Cancel {
        id: u64,
    },
    /// The bytes of a file asked for, or none: the page gets a 404.
    File {
        token: u64,
        found: bool,
    },
}

/// Why a render came to nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FailKind {
    /// The block's own code threw, did not load or never finished.
    Script,
    /// The page took more time than a render has; the helper exits after saying so.
    Hung,
    /// Taller or larger than an image may be, or of no height.
    Size,
    /// The engine would not draw it.
    Snapshot,
    /// The page's process died.
    Crashed,
    Cancelled,
}

impl FailKind {
    fn name(self) -> &'static str {
        match self {
            FailKind::Script => "script",
            FailKind::Hung => "hung",
            FailKind::Size => "size",
            FailKind::Snapshot => "snapshot",
            FailKind::Crashed => "crashed",
            FailKind::Cancelled => "cancelled",
        }
    }

    fn from_name(s: &str) -> Option<FailKind> {
        Some(match s {
            "script" => FailKind::Script,
            "hung" => FailKind::Hung,
            "size" => FailKind::Size,
            "snapshot" => FailKind::Snapshot,
            "crashed" => FailKind::Crashed,
            "cancelled" => FailKind::Cancelled,
            _ => return None,
        })
    }
}

/// An image's measures. The bytes of the frame are RGBA8, premultiplied, sRGB, top row first, with
/// no gap between rows.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rendered {
    pub id: u64,
    /// The block's height in CSS pixels.
    pub css_height: f64,
    pub px_width: u32,
    pub px_height: u32,
    /// Milliseconds from the helper taking it up to the image being ready.
    pub ms: f64,
}

/// From the helper to the client.
#[derive(Clone, Debug, PartialEq)]
pub enum FromHelper {
    /// The first frame: the helper is up. `available` is false on a platform with no web engine.
    Hello {
        protocol: u32,
        available: bool,
    },
    /// The page asks for a file. The path is what the page wrote, decoded: untrusted.
    NeedFile {
        token: u64,
        id: u64,
        area: Area,
        path: String,
    },
    Rendered(Rendered),
    Failed {
        id: u64,
        kind: FailKind,
        reason: String,
    },
}

fn get_u64(o: &Map<String, Value>, k: &str) -> io::Result<u64> {
    o.get(k).and_then(Value::as_u64).ok_or_else(|| bad(format!("no {k}")))
}

fn get_str<'a>(o: &'a Map<String, Value>, k: &str, max: usize) -> io::Result<&'a str> {
    let s = o.get(k).and_then(Value::as_str).ok_or_else(|| bad(format!("no {k}")))?;
    if s.len() > max {
        return Err(bad(format!("{k} too long")));
    }
    Ok(s)
}

fn is_var_name(s: &str) -> bool {
    s.len() <= 64
        && s.strip_prefix("--")
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'))
}

impl RenderRequest {
    /// Whether a helper may take it up. Both sides check: a request is bounded before it is sent.
    pub fn validate(&self) -> io::Result<()> {
        let ok = WIDTHS.contains(&self.width)
            && SCALES.contains(&self.scale)
            && LIMITS_MS.contains(&self.limit_ms)
            && !self.session.is_empty()
            && self.session.len() <= 128
            && self.lang.len() <= 64
            && self.html.len() <= MAX_HTML
            && self.vars.len() <= MAX_VARS
            && self.vars.iter().all(|(k, v)| is_var_name(k) && v.len() <= 1024)
            && self.styles.len() <= MAX_STYLES
            && self
                .styles
                .iter()
                .chain(&self.module)
                .all(|p| !p.is_empty() && p.len() <= MAX_PATH);
        if ok {
            Ok(())
        } else {
            Err(bad("a render request out of bounds"))
        }
    }
}

impl ToHelper {
    pub fn encode(&self) -> Value {
        match self {
            ToHelper::Render(r) => json!({
                "t": "render", "id": r.id, "session": r.session, "width": r.width, "scale": r.scale,
                "dark": r.dark, "lang": r.lang, "vars": r.vars, "html": r.html, "styles": r.styles,
                "module": r.module, "limit_ms": r.limit_ms,
            }),
            ToHelper::Cancel { id } => json!({"t": "cancel", "id": id}),
            ToHelper::File { token, found } => json!({"t": "file", "token": token, "found": found}),
        }
    }

    pub fn decode(v: &Value) -> io::Result<ToHelper> {
        let o = v.as_object().ok_or_else(|| bad("not an object"))?;
        match get_str(o, "t", 16)? {
            "render" => {
                // counted and measured before anything of them is copied
                let list = |k: &str, max: usize| -> io::Result<&[Value]> {
                    let l = o
                        .get(k)
                        .and_then(Value::as_array)
                        .ok_or_else(|| bad(format!("no {k}")))?;
                    if l.len() > max {
                        return Err(bad(format!("{k}: too many")));
                    }
                    Ok(l)
                };
                let vars = list("vars", MAX_VARS)?
                    .iter()
                    .map(|p| match p.as_array().map(Vec::as_slice) {
                        Some([Value::String(k), Value::String(v)]) if k.len() <= 64 && v.len() <= 1024 => {
                            Ok((k.clone(), v.clone()))
                        }
                        _ => Err(bad("a variable of the wrong shape")),
                    })
                    .collect::<io::Result<Vec<_>>>()?;
                let styles = list("styles", MAX_STYLES)?
                    .iter()
                    .map(|s| match s.as_str() {
                        Some(s) if s.len() <= MAX_PATH => Ok(s.to_string()),
                        _ => Err(bad("a style of the wrong shape")),
                    })
                    .collect::<io::Result<Vec<_>>>()?;
                let module = match o.get("module") {
                    None | Some(Value::Null) => None,
                    Some(Value::String(s)) => Some(s.clone()),
                    Some(_) => return Err(bad("a module of the wrong shape")),
                };
                let r = RenderRequest {
                    id: get_u64(o, "id")?,
                    session: get_str(o, "session", 128)?.to_string(),
                    width: u32::try_from(get_u64(o, "width")?).map_err(|_| bad("width"))?,
                    scale: o.get("scale").and_then(Value::as_f64).ok_or_else(|| bad("no scale"))?,
                    dark: o.get("dark").and_then(Value::as_bool).ok_or_else(|| bad("no dark"))?,
                    lang: get_str(o, "lang", 64)?.to_string(),
                    vars,
                    html: get_str(o, "html", MAX_HTML)?.to_string(),
                    styles,
                    module,
                    limit_ms: u32::try_from(get_u64(o, "limit_ms")?).map_err(|_| bad("limit_ms"))?,
                };
                r.validate()?;
                Ok(ToHelper::Render(r))
            }
            "cancel" => Ok(ToHelper::Cancel { id: get_u64(o, "id")? }),
            "file" => Ok(ToHelper::File {
                token: get_u64(o, "token")?,
                found: o.get("found").and_then(Value::as_bool).ok_or_else(|| bad("no found"))?,
            }),
            other => Err(bad(format!("unknown message {other}"))),
        }
    }
}

impl FromHelper {
    pub fn encode(&self) -> Value {
        match self {
            FromHelper::Hello { protocol, available } => {
                json!({"t": "hello", "protocol": protocol, "available": available})
            }
            FromHelper::NeedFile { token, id, area, path } => {
                json!({"t": "need", "token": token, "id": id, "area": area.host(), "path": path})
            }
            FromHelper::Rendered(r) => json!({
                "t": "rendered", "id": r.id, "css_height": r.css_height, "px_width": r.px_width,
                "px_height": r.px_height, "ms": r.ms,
            }),
            FromHelper::Failed { id, kind, reason } => {
                json!({"t": "failed", "id": id, "kind": kind.name(), "reason": reason})
            }
        }
    }

    /// An image's measures have to be within the bounds: its bytes are then read by them.
    pub fn decode(v: &Value) -> io::Result<FromHelper> {
        let o = v.as_object().ok_or_else(|| bad("not an object"))?;
        match get_str(o, "t", 16)? {
            "hello" => Ok(FromHelper::Hello {
                protocol: u32::try_from(get_u64(o, "protocol")?).map_err(|_| bad("protocol"))?,
                available: o
                    .get("available")
                    .and_then(Value::as_bool)
                    .ok_or_else(|| bad("no available"))?,
            }),
            "need" => Ok(FromHelper::NeedFile {
                token: get_u64(o, "token")?,
                id: get_u64(o, "id")?,
                area: Area::from_host(get_str(o, "area", 16)?).ok_or_else(|| bad("an unknown area"))?,
                path: get_str(o, "path", MAX_PATH)?.to_string(),
            }),
            "rendered" => {
                let side = |k: &str| -> io::Result<u32> {
                    let n = get_u64(o, k)?;
                    if n == 0 || n > MAX_SIDE_PX as u64 {
                        return Err(bad(format!("{k} out of bounds")));
                    }
                    Ok(n as u32)
                };
                let (px_width, px_height) = (side("px_width")?, side("px_height")?);
                let pixels = px_width as u64 * px_height as u64;
                if pixels > MAX_PIXELS {
                    return Err(bad("an image over the bounds"));
                }
                let number = |k: &str| o.get(k).and_then(Value::as_f64).ok_or_else(|| bad(format!("no {k}")));
                let css_height = number("css_height")?;
                if !css_height.is_finite() || css_height <= 0.0 || css_height > MAX_SIDE_PX as f64 * 2.0 {
                    return Err(bad("css_height out of bounds"));
                }
                Ok(FromHelper::Rendered(Rendered {
                    id: get_u64(o, "id")?,
                    css_height,
                    px_width,
                    px_height,
                    ms: number("ms")?,
                }))
            }
            "failed" => Ok(FromHelper::Failed {
                id: get_u64(o, "id")?,
                kind: FailKind::from_name(get_str(o, "kind", 16)?).ok_or_else(|| bad("an unknown kind"))?,
                reason: get_str(o, "reason", 4096)?.to_string(),
            }),
            other => Err(bad(format!("unknown message {other}"))),
        }
    }
}
