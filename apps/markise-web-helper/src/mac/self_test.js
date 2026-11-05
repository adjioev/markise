// Each attempt ends in 'stopped' or 'managed'; a cell of that colour stands for it. The address
// below is a listener of the self-test: it must hear nothing, whatever any cell says.
const N = 'http://TARGET';
const wait = (ms) => new Promise((r) => setTimeout(r, ms));
const timed = (p, ms = 400) => Promise.race([p, wait(ms).then(() => 'timeout')]);
const loads = (make) => timed(new Promise((res) => {
  const e = make();
  e.addEventListener('load', () => res('managed'));
  e.addEventListener('error', () => res('stopped'));
  document.body.append(e);
}));
const nothing = async (f) => { f(); await wait(120); return 'stopped'; };
const refused = async (p) => { try { await p(); return 'managed'; } catch { return 'stopped'; } };
const stopped = (r) => r === 'stopped' || r === 'timeout';
const el = (tag, props) => Object.assign(document.createElement(tag), props);
const add = (e) => { document.body.append(e); return e; };
const attempts = [
  () => refused(() => fetch(N + '/fetch')),
  () => new Promise((res) => { const x = new XMLHttpRequest(); x.onload = () => res('managed'); x.onerror = () => res('stopped'); x.open('GET', N + '/xhr'); x.send(); }),
  () => loads(() => el('img', { src: N + '/img' })),
  () => loads(() => el('script', { src: N + '/script.js' })),
  () => loads(() => el('link', { rel: 'stylesheet', href: N + '/s.css' })),
  () => refused(() => new FontFace('x', `url(${N}/f.woff)`).load()),
  () => nothing(() => { const d = add(el('div')); d.style.background = `url(${N}/bg.png)`; d.style.width = d.style.height = '4px'; }),
  () => nothing(() => add(el('iframe', { src: N + '/frame' }))),
  () => loads(() => el('img', { src: 'data:image/svg+xml,' + encodeURIComponent(`<svg xmlns="http://www.w3.org/2000/svg" width="4" height="4"><image href="${N}/in-svg.png" width="4" height="4"/></svg>`) })).then(() => 'stopped'),
  () => timed(new Promise((res) => { const s = new WebSocket(N.replace('http', 'ws') + '/ws'); s.onopen = () => res('managed'); s.onerror = () => res('stopped'); })),
  () => timed(new Promise((res) => { const s = new EventSource(N + '/events'); s.onopen = () => res('managed'); s.onerror = () => res('stopped'); })),
  () => nothing(() => navigator.sendBeacon(N + '/beacon', 'x')),
  () => nothing(() => add(el('video', { src: N + '/v.mp4', preload: 'auto' }))),
  () => nothing(() => add(el('audio', { src: N + '/a.mp3', preload: 'auto' }))),
  () => refused(() => fetch('file:///etc/hosts')),
  () => loads(() => el('img', { src: 'file:///System/Library/CoreServices/Finder.app/Contents/Resources/Finder.icns' })),
  () => nothing(() => add(el('iframe', { src: 'file:///etc/hosts' }))),
  () => loads(() => el('script', { src: 'file:///etc/hosts' })),
  () => nothing(() => { location.href = N + '/navigate'; }),
  () => nothing(() => { location.replace(N + '/replace'); }),
  () => nothing(() => { document.head.append(el('meta', { httpEquiv: 'refresh', content: `0;url=${N}/refresh` })); }),
  () => nothing(() => add(el('a', { href: N + '/click' })).click()),
  () => nothing(() => add(el('a', { href: N + '/click-blank', target: '_blank' })).click()),
  () => nothing(() => add(el('a', { href: '#', ping: N + '/ping' })).click()),
  () => nothing(() => add(el('a', { href: 'mailto:nobody@invalid' })).click()),
  () => nothing(() => { location.href = 'markise-self-test-app://open'; }),
  () => nothing(() => add(el('form', { method: 'post', action: N + '/form' })).submit()),
  async () => (window.open(N + '/window') ? 'managed' : 'stopped'),
  () => refused(async () => { const w = new Worker(URL.createObjectURL(new Blob([`fetch('${N}/worker')`]))); await wait(100); w.terminate(); throw 0; }),
  () => refused(() => navigator.serviceWorker.register('sw.js')),
  () => nothing(() => { for (const rel of ['preconnect', 'dns-prefetch']) document.head.append(el('link', { rel, href: N + '/pre' })); }),
  () => nothing(() => document.head.append(el('link', { rel: 'prefetch', href: N + '/prefetch' }))),
  async () => (typeof RTCPeerConnection === 'undefined' ? 'stopped' : 'managed'),
  async () => { const f = add(el('iframe')); const w = f.contentWindow; return w && typeof w.RTCPeerConnection !== 'undefined' ? 'managed' : 'stopped'; },
  async () => { if (typeof WebTransport === 'undefined') return 'stopped'; return refused(async () => { const t = new WebTransport(N.replace('http', 'https') + '/wt'); await timed(t.ready); throw 0; }); },
  () => nothing(() => add(el('iframe', { src: `data:text/html,<script>fetch('${N}/data-frame')<\/script>` }))),
  () => nothing(() => add(el('iframe', { src: URL.createObjectURL(new Blob([`<script>fetch('${N}/blob-frame')<\/script>`], { type: 'text/html' })) }))),
  () => nothing(() => { const f = add(el('iframe')); const d = f.contentDocument; if (d) { const a = d.createElement('a'); a.href = N + '/frame-blank'; a.target = '_blank'; d.documentElement.append(a); a.click(); } }),
  () => refused(async () => { eval('1'); }),
  async () => { alert('a'); const c = confirm('c'); const p = prompt('p'); return c === false && p === null ? 'stopped' : 'managed'; },
  () => nothing(() => add(el('input', { type: 'file' })).click()),
  () => refused(async () => { if (!navigator.mediaDevices) throw 0; await navigator.mediaDevices.getUserMedia({ audio: true }); }),
];
export default async function init(root) {
  const cells = document.createElement('div');
  cells.style.cssText = 'display:flex;flex-wrap:wrap;width:400px';
  root.append(cells);
  for (const a of attempts) {
    let r;
    try { r = await a(); } catch { r = 'stopped'; }
    const c = document.createElement('div');
    c.style.cssText = `width:CELLpx;height:CELLpx;flex:none;background:${stopped(r) ? 'rgb(0,255,0)' : 'rgb(255,0,0)'}`;
    cells.append(c);
  }
  await wait(100);
}
