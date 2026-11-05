//! What runs in the page besides a package's module. These scripts are constants: everything of
//! a document reaches them as an argument, never as script text.

/// The CSP of the page, in its response and its markup. Nothing but the scheme loads, a script
/// can only be a package's file, no inline script or handler in a document's markup runs, and
/// nothing can post or frame anything.
pub const CSP: &str = "default-src 'none'; script-src markise-block:; style-src markise-block: 'unsafe-inline'; \
     img-src markise-block: data: blob:; font-src markise-block: data:; connect-src markise-block:; \
     worker-src 'none'; base-uri markise-block:; form-action 'none'; frame-ancestors 'none'";

/// Whatever else blocks a load, this does too, whatever the page does (a CSP in a package's own
/// meta tag could only add to the one above).
pub const RULES: &str = r#"[
  {"trigger":{"url-filter":".*"},"action":{"type":"block"}},
  {"trigger":{"url-filter":"^markise-block://"},"action":{"type":"ignore-previous-rules"}}
]"#;

pub fn shell() -> String {
    format!(
        r#"<!doctype html><html><head><meta charset="utf-8">
<meta http-equiv="Content-Security-Policy" content="{CSP}">
<base href="markise-block://document/">
<style>
html,body{{margin:0;padding:0;background:transparent;overflow:hidden;max-width:none;width:auto;min-width:0}}
*,*::before,*::after{{animation:none!important;transition:none!important;caret-color:transparent}}
#markise-root{{display:flow-root}}
#markise-root>*{{margin-top:0!important;margin-bottom:0!important}}
</style></head><body><div id="markise-root"></div></body></html>"#
    )
}

/// In a world of its own: the theme, the style sheets and the markup, all from `req`. A style
/// sheet that does not come in time does not hold the block back.
pub const PREPARE: &str = r#"
if ('RTCPeerConnection' in window || 'webkitRTCPeerConnection' in window) throw new Error('peer connections are on');
if ('mediaDevices' in navigator) throw new Error('capture is on');
// nothing here is interactive: no click anyone sends gets to what a click does
window.addEventListener('click', (e) => e.preventDefault(), true);
window.addEventListener('submit', (e) => e.preventDefault(), true);
const r = JSON.parse(req);
const sleep = (ms) => new Promise((done) => setTimeout(done, ms));
const de = document.documentElement;
if (r.lang) de.lang = r.lang;
de.dataset.theme = r.dark ? 'dark' : 'light';
de.style.colorScheme = r.dark ? 'dark' : 'light';
for (const [name, value] of r.vars) de.style.setProperty(name, value);
const sheets = r.styles.map((href) => new Promise((done) => {
  const link = document.createElement('link');
  link.rel = 'stylesheet';
  link.onload = link.onerror = () => done();
  link.href = href;
  document.head.append(link);
}));
await Promise.race([Promise.all(sheets), sleep(1500)]);
const root = document.getElementById('markise-root');
// Left to itself the engine does not hold a paint back for a large picture: it asks for it to be
// decoded and goes on, and a snapshot shows the frame with nothing in it. Set before the picture
// loads, or it does not help (measured): now for what is in the markup, and as soon as they appear for
// what the block's code adds.
const whole = (i) => { i.loading = 'eager'; i.decoding = 'sync'; };
const pictures = (node) => node.querySelectorAll ? [...(node.matches('img') ? [node] : []), ...node.querySelectorAll('img')] : [];
new MutationObserver((changes) => {
  for (const c of changes) for (const n of c.addedNodes) pictures(n).forEach(whole);
}).observe(root, { childList: true, subtree: true });
root.innerHTML = r.html;
pictures(root).forEach(whole);
return document.querySelectorAll('#markise-root > *').length;
"#;

/// In the page's world, as the module expects: its default export gets the block. The URL is an
/// argument. This is the only thing the helper asks of a page's code.
pub const RUN_MODULE: &str = r#"
const m = await import(url);
const root = document.querySelector('#markise-root > *');
if (typeof m.default === 'function' && root) await m.default(root);
return true;
"#;

/// In the world of its own again: waits for pictures and fonts, within bounds, and measures. A page
/// resizing itself gets one measure: there is no second look. The answer is three numbers: the
/// height, the pictures there are, and the pictures that did not come. A picture with nothing to
/// fetch (`data:`, `blob:`, no address) is the page's own business: only what was asked for counts.
pub const SETTLE: &str = r#"
const sleep = (ms) => new Promise((done) => setTimeout(done, ms));
const root = document.getElementById('markise-root');
// what a picture's own load adds is waited for too, within the one bound
const seen = new Set();
const fresh = () => [...root.querySelectorAll('img')].filter((i) => !seen.has(i));
const until = performance.now() + 1500;
for (let now = fresh(); now.length && performance.now() < until; now = fresh()) {
  now.forEach((i) => seen.add(i));
  await Promise.race([
    Promise.all(now.filter((i) => !i.complete).map((i) => new Promise((done) => {
      i.addEventListener('load', done, { once: true });
      i.addEventListener('error', done, { once: true });
    }))),
    sleep(until - performance.now()),
  ]);
}
await Promise.race([
  Promise.all([...root.querySelectorAll('img')].map((i) => i.decode().catch(() => {}))),
  sleep(1500),
]);
// a CSS background: decoded as a picture of the same file is, within the same bound
const backgrounds = new Set();
for (const e of [...root.querySelectorAll('*')].slice(0, 2000)) {
  for (const pseudo of [null, '::before', '::after']) {
    for (const m of getComputedStyle(e, pseudo).backgroundImage.matchAll(/url\("([^"]+)"\)/g)) backgrounds.add(m[1]);
  }
}
await Promise.race([
  Promise.all([...backgrounds].map((u) => {
    const i = new Image();
    i.decoding = 'sync';
    i.src = u;
    return i.decode().catch(() => {});
  })),
  sleep(1500),
]);
await Promise.race([document.fonts.ready, sleep(1000)]);
const images = [...root.querySelectorAll('img')];
const chosen = (i) => i.currentSrc || i.getAttribute('src') || i.getAttribute('srcset');
const asked = (i) => chosen(i) && !/^(data|blob):/i.test(chosen(i));
const broken = images.filter((i) => asked(i) && i.complete && !(i.naturalWidth > 0)).length;
const open = images.filter((i) => asked(i) && !i.complete).length;
return `${root.getBoundingClientRect().height} ${images.length} ${broken + open}`;
"#;

/// In the same world, last thing before an image goes out: the pictures there are now, the ones that
/// failed, the ones still on their way. The page may have added some since it settled.
pub const CHECK: &str = r#"
const images = [...document.querySelectorAll('#markise-root img')];
const chosen = (i) => i.currentSrc || i.getAttribute('src') || i.getAttribute('srcset');
const asked = images.filter((i) => chosen(i) && !/^(data|blob):/i.test(chosen(i)));
const broken = asked.filter((i) => i.complete && !(i.naturalWidth > 0)).length;
const open = asked.filter((i) => !i.complete).length;
return `${images.length} ${broken} ${open}`;
"#;
