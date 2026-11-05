//! What a peer may send: frames and messages are untrusted both ways.

use std::io::Cursor;

use markise_web_render::proto::*;
use serde_json::json;

fn request() -> RenderRequest {
    RenderRequest {
        id: 7,
        session: "s".to_string(),
        width: 600,
        scale: 2.0,
        dark: true,
        lang: "en".to_string(),
        vars: vec![("--markise-text".to_string(), "#111".to_string())],
        html: "<section>\"'</section>".to_string(),
        styles: vec!["styles/styles.css".to_string()],
        module: Some("components/a/a.js".to_string()),
        limit_ms: 5000,
    }
}

fn frame(json: &[u8], bytes: &[u8]) -> Vec<u8> {
    let mut out = (json.len() as u32).to_le_bytes().to_vec();
    out.extend_from_slice(json);
    out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(bytes);
    out
}

#[test]
fn messages_survive_the_trip() {
    let mut out = Vec::new();
    let to = ToHelper::Render(request());
    write_frame(&mut out, &to.encode(), &[]).unwrap();
    let file = ToHelper::File { token: 3, found: true };
    write_frame(&mut out, &file.encode(), b"bytes").unwrap();
    let mut r = Cursor::new(out);
    assert_eq!(read_to_helper(&mut r).unwrap(), (to, Vec::new()));
    assert_eq!(read_to_helper(&mut r).unwrap(), (file, b"bytes".to_vec()));

    let mut out = Vec::new();
    let rendered = FromHelper::Rendered(Rendered {
        id: 7,
        css_height: 2.0,
        px_width: 2,
        px_height: 4,
        ms: 1.5,
    });
    write_frame(&mut out, &rendered.encode(), &[0; 32]).unwrap();
    let need = FromHelper::NeedFile {
        token: 1,
        id: 7,
        area: Area::Document,
        path: "a/b.png".to_string(),
    };
    write_frame(&mut out, &need.encode(), &[]).unwrap();
    let failed = FromHelper::Failed {
        id: 7,
        kind: FailKind::Hung,
        reason: "why".to_string(),
    };
    write_frame(&mut out, &failed.encode(), &[]).unwrap();
    let mut r = Cursor::new(out);
    assert_eq!(read_from_helper(&mut r).unwrap(), (rendered, vec![0; 32]));
    assert_eq!(read_from_helper(&mut r).unwrap().0, need);
    assert_eq!(read_from_helper(&mut r).unwrap().0, failed);
}

#[test]
fn a_false_length_costs_nothing_and_is_an_error() {
    // over the bound: refused before any byte of it
    let mut over = ((MAX_JSON + 1) as u32).to_le_bytes().to_vec();
    over.extend_from_slice(b"{}");
    assert!(read_to_helper(&mut Cursor::new(over)).is_err());
    // within the bound, but the bytes are not there
    let mut short = ((MAX_JSON - 1) as u32).to_le_bytes().to_vec();
    short.extend_from_slice(b"{}");
    assert!(read_to_helper(&mut Cursor::new(short)).is_err());
    assert!(read_to_helper(&mut Cursor::new(Vec::new())).is_err());
}

#[test]
fn a_little_message_has_no_attachment() {
    let need = json!({"t": "failed", "id": 1, "kind": "hung", "reason": ""}).to_string();
    let f = frame(need.as_bytes(), b"x");
    assert!(read_from_helper(&mut Cursor::new(f)).is_err());
    let cancel = json!({"t": "cancel", "id": 1}).to_string();
    let f = frame(cancel.as_bytes(), b"x");
    assert!(read_to_helper(&mut Cursor::new(f)).is_err());
}

#[test]
fn json_of_the_wrong_shape_is_an_error() {
    for bad in [
        &b"not json"[..],
        b"[]",
        b"{}",
        br#"{"t":"unknown"}"#,
        br#"{"t":"cancel"}"#,
        br#"{"t":"cancel","id":"1"}"#,
        br#"{"t":"file","token":1}"#,
        br#"{"t":"render","id":1}"#,
    ] {
        assert!(read_to_helper(&mut Cursor::new(frame(bad, b""))).is_err(), "{bad:?}");
    }
}

#[test]
fn a_request_out_of_bounds_is_refused() {
    let bad: Vec<fn(&mut RenderRequest)> = vec![
        |r| r.width = 0,
        |r| r.width = 100_000,
        |r| r.scale = 0.0,
        |r| r.scale = f64::MAX,
        |r| r.limit_ms = 0,
        |r| r.limit_ms = 1_000_000,
        |r| r.session = String::new(),
        |r| r.html = "x".repeat(MAX_HTML + 1),
        |r| r.vars = vec![("not-a-variable".to_string(), "x".to_string())],
        |r| r.vars = vec![("--a;b".to_string(), "x".to_string())],
        |r| r.vars = vec![("--a".to_string(), "x".repeat(2000))],
        |r| r.styles = vec![String::new()],
        |r| r.styles = vec!["a.css".to_string(); MAX_STYLES + 1],
        |r| r.module = Some("x".repeat(MAX_PATH + 1)),
    ];
    for change in bad {
        let mut r = request();
        change(&mut r);
        assert!(r.validate().is_err(), "{r:?}");
        let f = frame(ToHelper::Render(r).encode().to_string().as_bytes(), b"");
        assert!(read_to_helper(&mut Cursor::new(f)).is_err());
    }
}

#[test]
fn an_image_has_to_be_what_its_measures_say() {
    let msg = |w: u64, h: u64, css: f64| {
        json!({"t": "rendered", "id": 1, "css_height": css, "px_width": w, "px_height": h, "ms": 1.0}).to_string()
    };
    let ok = frame(msg(2, 2, 2.0).as_bytes(), &[0; 16]);
    assert!(read_from_helper(&mut Cursor::new(ok)).is_ok());
    for bad in [
        frame(msg(2, 2, 2.0).as_bytes(), &[0; 15]),
        frame(msg(0, 2, 2.0).as_bytes(), b""),
        frame(msg(2, 0, 2.0).as_bytes(), b""),
        frame(msg(MAX_SIDE_PX as u64 + 1, 1, 1.0).as_bytes(), b""),
        frame(msg(8000, 8000, 1.0).as_bytes(), b""),
        frame(msg(2, 2, 0.0).as_bytes(), &[0; 16]),
        frame(msg(2, 2, 1e12).as_bytes(), &[0; 16]),
    ] {
        assert!(read_from_helper(&mut Cursor::new(bad)).is_err());
    }
}

#[test]
fn urls_name_files_and_nothing_else() {
    assert_eq!(
        parse_file_url("markise-block://package/styles/a.css"),
        Some((Area::Package, "styles/a.css".to_string()))
    );
    assert_eq!(
        parse_file_url("markise-block://document/a%20b.png?x=1#y"),
        Some((Area::Document, "a b.png".to_string()))
    );
    // decoded, and still for the client to refuse
    assert_eq!(
        parse_file_url("markise-block://document/..%2Fsecret"),
        Some((Area::Document, "../secret".to_string()))
    );
    for bad in [
        "https://package/a",
        "file:///etc/hosts",
        "markise-block://other/a",
        "markise-block://package",
        "markise-block://package/",
        "markise-block://package/%zz",
        "markise-block://package/%ff",
    ] {
        assert_eq!(parse_file_url(bad), None, "{bad}");
    }
    let url = file_url(Area::Document, "a b/é.png");
    assert_eq!(url, "markise-block://document/a%20b/%C3%A9.png");
    assert_eq!(parse_file_url(&url), Some((Area::Document, "a b/é.png".to_string())));
}

/// Panics if anything reads past what it was given: a read that should have been refused before it.
struct NoMore(Cursor<Vec<u8>>);

impl std::io::Read for NoMore {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.0.read(buf)?;
        assert!(
            n > 0 || buf.is_empty(),
            "read past the header of an attachment that must be refused"
        );
        Ok(n)
    }
}

fn header_only(json: &str, len: u32) -> NoMore {
    let mut out = (json.len() as u32).to_le_bytes().to_vec();
    out.extend_from_slice(json.as_bytes());
    out.extend_from_slice(&len.to_le_bytes());
    NoMore(Cursor::new(out))
}

#[test]
fn an_attachment_is_refused_by_its_message_before_a_byte_of_it_is_read() {
    let rendered = |w: u64, h: u64| {
        json!({"t": "rendered", "id": 1, "css_height": 2.0, "px_width": w, "px_height": h, "ms": 1.0}).to_string()
    };
    // measures out of bounds, with the largest attachment there is
    assert!(read_from_helper(&mut header_only(&rendered(0, 2), 96_000_000)).is_err());
    // fine measures, and a length that is not theirs
    assert!(read_from_helper(&mut header_only(&rendered(2, 2), 96_000_000)).is_err());
    assert!(read_from_helper(&mut header_only(&rendered(2, 2), 15)).is_err());
    // an unknown message
    assert!(read_from_helper(&mut header_only(r#"{"t":"nope"}"#, 1)).is_err());
    // to the helper: a file with none found has no bytes
    let none = json!({"t": "file", "token": 1, "found": false}).to_string();
    assert!(read_to_helper(&mut header_only(&none, 5)).is_err());
    let over = json!({"t": "file", "token": 1, "found": true}).to_string();
    assert!(read_to_helper(&mut header_only(&over, MAX_FILE as u32 + 1)).is_err());
}

#[test]
fn lists_are_counted_before_anything_of_them_is_copied() {
    // a list far over its bound comes back as an error with no entry looked at beyond the bound
    let mut r = ToHelper::Render(request()).encode();
    r["styles"] = serde_json::Value::Array(vec![json!("a"); MAX_STYLES + 1]);
    assert!(ToHelper::decode(&r).is_err());
    let mut r = ToHelper::Render(request()).encode();
    r["vars"] = serde_json::Value::Array(vec![json!(["--a", "b"]); MAX_VARS + 1]);
    assert!(ToHelper::decode(&r).is_err());
    // a long entry too
    let mut r = ToHelper::Render(request()).encode();
    r["styles"] = json!(["x".repeat(MAX_PATH + 1)]);
    assert!(ToHelper::decode(&r).is_err());
    let mut r = ToHelper::Render(request()).encode();
    r["vars"] = json!([["--a", "x".repeat(4096)]]);
    assert!(ToHelper::decode(&r).is_err());
}

#[test]
fn a_picture_that_did_not_come_has_a_kind_of_its_own() {
    let failed = FromHelper::Failed {
        id: 7,
        kind: FailKind::Images,
        reason: "why".to_string(),
    };
    let mut out = Vec::new();
    write_frame(&mut out, &failed.encode(), &[]).unwrap();
    assert_eq!(read_from_helper(&mut Cursor::new(out)).unwrap().0, failed);
}
