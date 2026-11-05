# ADR 0023: blocks drawn by their site's code, in a helper

Status: accepted for issue #178, phase 6. macOS; Windows and Linux come in phase 7.

## Context

A block with a native view shows it (ADR 0021). Others, scenes and interactive mechanics, are what a site's
own code draws, and Markise must never reimplement them: it keeps no vocabulary of any site. So a package's
JS and CSS (ADR 0019) draw the block, and Markise shows the result as a picture in its place. That is
untrusted code, run after a consent that was about bytes, not about who made them. Whatever it does, it must
not reach the network, the user's files, other packages or the editor, and it must not stall anything.

**The owner's decisions.** A block with a `view` shows natively, one without shows the site's picture, and no
package, or a helper unavailable or failing, shows the generic card. The drawing happens in a generic helper shipped with
Markise, HTML + CSS + JS to an image, on the system's web engine, with no installed browser needed, reusable for
plugins later (#171). It shares no window or surface with the editor.

## Decision

### Two parts

- **`markise-web-render`** (L0): the protocol and the client. It knows no web engine, no package and no Markdown.
- **`markise-web-helper`** (an app): one process that draws. On a platform with no engine wired up it says so in
  its first frame and exits, and that is all it does there.

### The protocol

Requests on the helper's stdin, answers on its stdout, stderr unused. A frame is `u32 LE` + JSON, `u32 LE` +
bytes. **Each side treats what it reads as untrusted**, since a page's code runs behind the helper:

- A length over its bound ends the conversation before anything is read: JSON 4 MiB; bytes by the message's kind, read
  only after the JSON says which, so a little message has no attachment (32 MiB for a file, 96 MiB for an image, nothing for
  the rest). Memory grows as bytes come, so a false length costs nothing.
- Messages are decoded by hand against their shape, with a bound on every value. An unknown one is an error. A bad frame
  ends the helper, or makes the client kill it.
- An image is RGBA8, premultiplied, sRGB, top row first, with no gap between rows: its length is its measures, and those
  are bounded. No image decoder ever sees the helper's output.

| To the helper | |
|---|---|
| `render` | id, session, width, scale, dark, language, CSS variables, markup, style sheets, module, time limit |
| `cancel` | spend no more on this render; a `failed` answers it |
| `file` | the bytes of a file asked for, or that there is none |

| From the helper | |
|---|---|
| `hello` | the first frame: the protocol's number, and whether this helper can draw at all |
| `need` | a page asks for a file: token, render, area and path, as written, decoded and **untrusted** |
| `rendered` | the block's height, the image's measures, the time taken; the image as bytes |
| `failed` | script, hung, size, snapshot, crashed or cancelled, with a reason in words |

### How a package draws a block

The format did not say, so this is the smallest generic contract, from the package format's own layout (ADR 0019). It
needs no new format name: these are files that format calls opaque, and nothing in it changes.

- **Styles:** `styles/styles.css` if there is one, then `components/<name>/<name>.css` if there is one, in that order.
  What they import comes the same way.
- **Script:** `components/<name>/<name>.js` if there is one, an ES module. Its default export gets the block's element
  and may return a promise; the picture is taken when that resolves. A block with no script draws from its styles alone.
- **The element:** generic markup from the block's fields (ADR 0020), by field type alone:

  ```html
  <section data-component="name" class="name" data-variant="word" data-argument="word">
    <h2 data-field="title">…</h2>                      <!-- heading, of its level -->
    <div data-field="body"><p>…</p></div>              <!-- text -->
    <blockquote data-field="note"><p>…</p></blockquote><!-- quote -->
    <ul data-field="steps"><li data-item>…</li></ul>   <!-- list -->
    <ul data-field="answers" data-select="single">     <!-- choices: single, or multiple -->
      <li data-item data-option-id="a1" data-correct="true">…</li>
    </ul>
    <div data-field="pictures"><img src="…" alt="…"></div> <!-- image -->
  </section>
  ```

  `data-variant` is there when the arguments name a variant the schema declares, `data-argument` when the schema declares an
  `argument`. A field with nothing in it has no element. Text inside goes through the HTML export with its sanitizer.
  Trailing ids (ADR 0018) are not text.
- **The page around it** is the same for all: `<html lang data-theme="light|dark">`, the variables below, and the block as
  the first element of a container that is as wide as the block's column. Its outer margins are the document's business,
  so they are zero; animations and transitions are off.

A package made for this needs nothing else. If a package's code cannot draw from this, it draws nothing and the block
stays a card: a package learns of that from its own code, not from anything here.

### Isolation

A package's script is untrusted code. What follows holds for that code and for a document's markup alike.

**It runs only for a package the user accepted.** There is no consent, and no helper start, otherwise: the client gets its
jobs only from a package in use (ADR 0019).

**A process of its own, nobody's window.** The helper never activates (`NSApplicationActivationPolicyProhibited`), and its
window is never ordered in, so nothing can appear on any screen, whatever the page does. WebKit is happy to snapshot in
such a window, at any scale asked for (checked at 1× and 3× on a 2× screen). A page nobody sees would have its timers held to a second or
more and could have its process held back, so the inactive scheduling policy and the engine's throttling features are off (they made a render run out of its
limit on a CI runner, doing nothing). The whole process is confined by `sandbox_init` once it starts:
no network, no reading the home folder, no writing outside the temporary folders. It is not an App Sandbox: that needs
a container tied to a signature, and an ad-hoc signature that changes with every build would make macOS ask the user about
the helper's own data. WebKit's own processes have sandboxes of their own.

**Nothing kept.** A web view per session, `WKWebsiteDataStore.nonPersistentDataStore`. A session is one package and one
document's folder, so no storage is shared between packages, and a package's code cannot carry a document's data into the
next document. Every render is a fresh document in that web view: what the last block's code left running does not go on.

**No network.** Each of these was found to be needed by a test page with a listener, which is why there are several:

| What | Stops |
|---|---|
| A compiled content rule list: everything but `^markise-block://` | fetch, XHR, pictures, scripts, styles, fonts, frames, media, WebSocket, EventSource, beacons, pings |
| A CSP in the page's response and in its markup | the same, once more; inline scripts and handlers in a document's markup; `eval`; workers; forms; `file:` |
| A navigation policy: the main frame to the exact URL the helper asked for, once | navigation, redirects, meta refresh, frames, downloads, other apps' URL schemes |
| `createWebView` answers nil, dialogs complete at once, capture and motion are denied, the open panel is empty | new windows, anything that waits for a person |
| The engine's features off, by name (`_setEnabled:forFeature:`): peer connections, a link's preconnect, DNS prefetch, prefetch, speculation rules, WebTransport | what no rule is asked about. Peer connections' UDP goes around everything else, and clicking a link preconnects before any policy, in any frame. Both were heard on a listener |
| Also off: capture, screen capture, speech recognition, geolocation, notifications, sharing, the async clipboard | what asks a person or the system, with nobody there. A capture request waited for ever on a CI runner, and could put a prompt on a user's screen |
| A check, before any package code runs, that a page has no peer connections and no capture | the engine ignoring those switches |
| A dead SOCKS proxy on the data store (`proxyConfigurations`, macOS 14), no failover | what is left at the level of the engine's networking: it resolves no name itself this way. It exempts loopback, and it did not catch a preconnect every time, so it is a second line and not the answer to one |
| A click and a submit stop at the window, before a page's listeners, in a world of its own | activating anything: nothing here is interactive |
| Safe browsing off | the engine's own questions about what loads |

The feature switches are private API, there being no public ones; the proxy needs macOS 14. **Without them there is no
helper**: it says `available: false`, and blocks stay cards. The check in the page also fails closed, so an OS that ignores the
switch draws nothing. This goes through Developer ID distribution, not the App Store, where private API would be refused.

An attempt that runs out of its own budget counts against the page, not for it: what did not finish was not seen to be stopped.

`markise-web-helper --self-test` is that test page: about forty attempts, aimed at this machine's address and then at loopback, at a
listener on all interfaces, TCP and UDP. The page cannot tell it anything, there being no way to, so it paints a cell per attempt and
the test reads the image.

**Files through the client, and nothing else.** The helper opens no file of a package or document. Its scheme handler
turns every load into a `need`, and the client decides:

- `package`: a clean relative path through `InstalledPackage::asset_path`, in a tree already checked to hold no links, opened
  by a handle that refuses to leave it.
- `document`: only paths on this block's own list of images, opened by a handle on the document's folder that refuses to
  leave it, links included, so a swap between check and open has nothing to gain.
- Anything else, or a file over the format's bound, gets a 404. No `file://` access is ever granted: the throwaway prototype's
  grant of whole-disk read is what this replaces. A budget per render bounds what a page can ask for: 512 files in the helper,
  and the client counts them and their bytes itself.
- A file is for the render that asked: each page has a URL of its own, and a load from any other page gets a 404.

**No bridge.** No script message handler, no way for a page to call anything. What leaves a page is its file requests and
its pixels. Both stay on this machine, and neither is treated as more than untrusted data: this must stay so.

**Data, never script.** All of a document reaches the page as a JSON string argument of `callAsyncJavaScript`, in a content
world of Markise's own, and goes into the page with `innerHTML`. The block's module is the only thing run in the page's
world, by its URL, as an argument. Measuring is in the own world, whose globals a page cannot touch; what comes out is a
number, checked.

**Bounds.**

| | |
|---|---|
| One render | 5 s by a timer in the helper that depends on nothing in the page; then it says `hung` and exits, and the page's process goes with it |
| The helper itself | the client's timer, a little longer, then `SIGKILL`, and it starts another when next needed, with a growing pause; after repeated failures with nothing drawn it counts as unavailable |
| A page done, nothing to do next | its document goes, so its code stops; if it does not within 1.5 s it is spinning, and the helper exits. A document at rest costs nothing |
| An image | 8192 px a side, 24 Mpx. A larger one is `size`, and the block stays a card. The block's height gets one measure, so a page resizing itself gets nowhere |
| Waiting for styles, pictures, fonts | 1.5 s, 1.5 s, 1 s, then the picture is taken as it stands |
| A block that fails | not asked again until its source, width, theme or scale changes |

**Accepted.** Memory a page allocates in its own process is bounded by time and by WebKit's own limit on such a process,
not by Markise: no public API sets one. The helper's stdin carries a client's messages and it trusts their order. A hash
is not a publisher (ADR 0019): code the user accepted can draw anything in its own picture, misleading included. It is
shown only as that block, in that block's place.

### Showing a picture

A picture changes nothing about a block's items (ADR 0021): they stay flat, pages split them as ever (ADR 0016), and every source
position, mark and edit goes on as it did. What changes is their heights.

- **The opener holds the picture** (`Prim::Picture`, drawn by the host, as an icon is). Its height is the picture's, scaled to the
  column if an older one stands in at another width. The items inside are hidden the way a closed disclosure's are, zero height
  and skipped by the caret. The closer stays with no height, so the gap after it is where a caret lands (issue #166). Nested blocks
  are inside the outermost picture. A picture draws its own edges, so its own card draws none; the block's outer margins are not in it,
  and the document's block spacing stands instead.
- **Which item is what** is read through `BlockViews`: `has_site`, then `site_shot`, once per version of a block, and `site_seen` for what is on
  screen, every frame. It answers from a map and never blocks. A document whose blocks nothing can draw pays one comparison per item.
- **A picture coming is a change of heights.** Above the viewport it moves nothing on screen, as any height does (ADR 0016). When the viewport starts inside the
  block, it keeps its relative place in the block. Only heights around the viewport are read at once; the others are corrected when they are laid out. A
  placeholder's measured height keeps what it held, for the views it was measured under; one never measured is estimated as cards.
- **Pending** shows the card, the first time. After that the newest picture of the same block stands in until its new one comes, so nothing
  flickers while typing above it, resizing, or after Save. "The same" is an identity the view keeps by opener and moves with every edit: an edit that goes over the opener, or another name
  or arguments there, ends it, so another block coming to stand at that offset shows its card, not this picture. The content is not compared: a block being edited differs from its picture by definition.
- **Failure** keeps the card, with a quiet reason on its first row (no `⚠`), in full on hover and for a screen reader. It is not tried again until the block changes.
- **What is shown which way.** A block that does not fit its schema has no picture: it shows what ADR 0021 says, native with its warning when it is only not filled
  in, a card when its structure is wrong. Only a block that fits can be expected to draw.
- **The caret.** A picture has no stops, so keys go past it. A click in its upper half lands on the place before it, in its lower half on the place after it, the
  virtual paragraphs of issue #166 where there is no text. A selection holding the block whole paints over the picture, and copies, deletes and moves it as any
  block. When a selection's end is inside (search, undo, Go to Heading through a caret), the block shows what it holds until it leaves. There is nothing to edit in a picture.
- **The pencil** stays: the form edits the block (ADR 0020), and Save is a new version, so a new picture. Over a picture the buttons show when the pointer is on it, on a ground of their own.
- **Known limit, as for cards at the top (ADR 0018):** with no gap offered before the document's first block, a click on a picture there goes to the next stop.

**The cache.** A picture is named by the package's hash and the document's folder (the session), the document's language, the block's name, arguments and source, its width, the
scale, and the theme's fingerprint. Pictures are textures, 256 MiB together, least recently used first, and none on disk. At most two become textures per frame. One over the graphics card's `max_texture_side` is refused and the block stays a card with that reason: scaling
would be work on the UI thread for a rare card, and tiles a lot of code for it. One gone is drawn
again when it is next on screen. Typing elsewhere changes no key, so it draws nothing.

**What the page sees of the theme,** all of it data on `<html>`:

| | |
|---|---|
| `data-theme` | `light` or `dark`; also `color-scheme` |
| `lang` | the `lang` key of the document's front matter, if it has one |
| `--markise-background`, `--markise-text`, `--markise-link`, `--markise-accent`, `--markise-selection` | the theme's colours |
| `--markise-font-size` | the theme's body size, in `px` |

The picture's background is transparent, so what a block does not paint is the document. A package with no dark look of its own draws what it draws, on the document's dark page.

**Scale.** The width is the column's in points, and the scale is the window's pixels per point, whole-window zoom included: at any zoom a picture is sharp, and it is another
picture. The helper's own screen has no say (ADR 0023, Isolation).

**Idle.** Nothing runs for a document at rest. `tell_client` speaks only when what is wanted changed; the client's thread sleeps with no timer unless a helper is up, and then wakes once
to let it go after two minutes; a page's document is gone when it is done; no frame is asked for.

### The switch

`BlockViewMode` goes Native, Site, Markdown (ADR 0021), per block and for the document, with the commands and the button that were there. What each state shows depends on what
a block can show (`BlockViewMode::shown`):

| State | A block with a view | A block with no view |
|---|---|---|
| Native | its view | its picture |
| Site | its picture | its picture |
| Markdown | the card | the card |

So a block with no view has two states, and a switch skips what shows the same (`next_for`). With nothing to draw with, Site is skipped everywhere: a block with no view then has
one state and no button, as before this phase. A state that shows what the document's state shows is no state of its own. Whether the last state should be the card or a block's raw text
stays an open question for the owner: this leaves what exists.

A picture being asked for from inside a block moves the caret to the place after it, since a block holding a selection's end shows what it holds. A block not filled in yet has no look
on any site: in the Site state it stays native with its warning. A block with a structural problem is a card in every state. The choice is kept as the other states are: per document, for the
session, by a live mark on the opener. A block just inserted starts in what its view allows, and shows its picture when the caret leaves it, if it has no view.

### Distribution

The helper ships on macOS only, as `Markise.app/Contents/MacOS/markise-web-helper`, next to the app's executable, which is where the app looks for it (`system_web_helpers`), in a
development build as much as in a bundle. A build that has none has no Site state, and its blocks stay cards. On Windows and Linux no helper is shipped, and the app does not look.

`scripts/package.sh` signs it before the bundle, as a bundle's signature covers what is in it. Ad-hoc today, as the app is; Developer ID and notarization come with the app's. Under hardened
runtime it needs no entitlement: it allocates no executable memory of its own, and WebKit's processes are the system's. It has no bundle of its own and needs none: the code sets its activation
policy before anything else, so it is never in the Dock, and no window of it is ever ordered in. It is built for the two architectures with the app. `cargo xtask dist`, the release workflow and CI's
full matrix all build and package it, and `scripts/smoke-web-helper.sh` mounts the disk image, verifies the signature and runs `--self-test` on the packaged helper: the fixture block and the forbidden page, on the runner's own engine.

The proxy's functions are looked up when it runs: the app starts on macOS 11, and linking what came with 14 would stop the helper before it could say it has no engine.

## Alternatives

- **A web view in the editor's process.** Its window, its run loop, its crash. The owner's decision excludes it.
- **Reading files in the helper, validated there.** A second implementation of the containment rules in the process nearest
  the untrusted code, and a process that needs read access at all.
- **A pure-Rust engine.** It would not run the site's script, which is the point (measured on the prototype).
- **`loadFileURL` with read access to a folder.** What the prototype did: the grant is a whole folder, and a `file:` origin.
- **A fresh web view and store per render.** About 115 ms against about 35 ms a render, measured, for what a fresh document and
  a session per document's folder already give.
- **App Sandbox with no network entitlement.** It would have closed the network at the OS level, but WebKit's page process
  stops at once under it (checked), and with the entitlement it closes nothing.

## Consequences

- A package can be drawn by Markise with no change, if it is built the way its site loads it.
- Markise's vocabulary stays the field types and the view kinds.
- A later OS can remove the private switch: then blocks with no view are cards until phase 6's check is revisited. Nothing breaks.
