# Rune API notes

> Notes taken while starting the app, when it was still called Cadence. The Rune API may have moved on since
> this commit; the code in `app/src` is the working reference.

**Rune commit: f3d04f3f2069d177d04c963b0ba9f6b2065e46eb** (branch `dev`, repo https://github.com/anomalyco/rune)

Everything below was read from `crates/rune/README.md`, `crates/rune-kit/README.md`, the examples and
`crates/rune/src` at that commit. Snippets were **not compiled** (the clone is read-only); signatures were
checked in source. Anything marked "(unverified)" was not confirmed. Paths are relative to the Rune repo.

---------------------------------------------------------------------------------------------------

## 1. App skeleton

```rust
use rune::{App, AppSpec, Cx, El, Tx, col, text};

struct Ui { /* all app state; NOT required to be Send */ }

fn init() -> Ui { Ui { } }
fn view(s: &Ui, cx: &mut Cx<Ui>) -> El { col().w_full().h_full().child(text("hi")) }

fn main() {
    rune::run(AppSpec::<Ui> {
        scenes: &[("album", |s| { /* mutate initial state */ })],
        ..AppSpec::new("Cadence", (1180.0, 760.0), init, view)   // (title, (w, h) in points, init, view)
    });
}
```

`AppSpec<S>` (crates/rune/src/tooling.rs). All function fields are **plain `fn` pointers, not closures**
(non-capturing closures coerce):

| field | type | notes |
|---|---|---|
| `title` | `&'static str` | window title; `Tx::set_title` changes it later |
| `size` | `(f32, f32)` | initial window size, points |
| `min_size` | `Option<(f32, f32)>` | default `None` = 420x400 |
| `init` | `fn() -> S` | first state |
| `view` | `fn(&S, &mut Cx<S>) -> El` | pure function of state; re-runs on every committed change |
| `scenes` | `&'static [(&str, fn(&mut S))]` | named states for `--scene` (see section 10) |
| `fonts` / `families` | `&[&[u8]]` / `(&str, &str)` | see section 2 |
| `setup` | `Option<fn(&mut S, Waker)>` | once, after state exists. `Waker = Arc<dyn Fn() + Send + Sync>`; calling it wakes the UI and runs `poll` |
| `poll` | `Option<fn(&mut S, f64, &mut Tx) -> bool>` | before every frame; f64 = seconds since start; **return true only if visible state changed** |
| `on_key` | `Option<fn(&mut S, &KeyEvent, &mut Tx) -> bool>` | keys not consumed by a focused control; true = consumed |

- `AppSpec::new(title, size, init, view)` fills the rest. **Always build as `AppSpec { .., ..AppSpec::new(..) }`**
  (the struct gains fields over time; AGENTS.md says adding pub fields breaks literals).
- **There is no `on_ready` in `AppSpec`.** `on_ready`, `on_close`, `on_window`, etc. are methods on
  `rune::App` (`App::new(spec).on_ready(|win: &WindowHandle| ..)`). Docs say: start slow work (network, child
  processes) from `on_ready`, after the first frame, and land early state with `tx.instant()`.
- `rune::run(spec)` = open one window, quit when it closes. `rune::App::new(spec)...run()` is the same with
  options and an `AppHandle` (section 6). Both read argv for the headless flags.
- **Cx<S>** (view time, read-only-ish context): `cx.system_dark()`, `cx.accent()`, `cx.titlebar_inset()`,
  `cx.layout_size("key") -> Option<[f32;2]>` (size of a keyed element from the last layout; re-runs the view only
  when the size changes), `cx.is_reduced_motion()`.
- **Tx** (handler/poll time, a transaction for the commit that follows): `tx.instant()` (no animation),
  `tx.animated()`, `tx.motion(spring)`, `tx.wake_after(Duration)` (schedule a poll), `tx.quit()`,
  `tx.close_window()`, `tx.set_title(..)`, `tx.focused()`, `tx.blur()`, scroll requests
  (`scroll_into_view(key)`, `scroll_to_bottom(key)`, `scroll_to(key,x,y)`, ...), `tx.copy(text)`, `tx.open_url(..)`.
- **Mutating state from handlers.** Handlers are `Fn` closures attached to elements; the closure parameter type
  must be annotated. They get `&mut S`, are queued and run between frames, then `view` re-runs and the diff animates:

```rust
el().on_press(|s: &mut Ui| s.playing = !s.playing)                    // no Tx
el().on_press_tx(|s: &mut Ui, tx: &mut Tx| { s.playing = !s.playing; tx.instant(); })
// capture owned data by move to identify the item (never an index into a virtual list):
let id = album.id.clone();
el().on_press(move |s: &mut Ui| s.open_album(&id))
```
  Hover/press visuals do not re-run the view (`.hover(color)`, `.press(color)`), so never keep hover in state.

---------------------------------------------------------------------------------------------------

## 2. Layout and styling primitives

Constructors: `el()` (a box, **flex row**), `row()`, `col()`, `text("..")`, `spacer()` (grow 1), `icon(&Icon, size)`,
`canvas(Shape)`, `text_input(&TextEdit, TextStyle)`. Builders are chained and consume `self`;
`.child(El)` / `.children(iter)`. Layout is Taffy (CSS flexbox), units are points (logical px).

| need | API |
|---|---|
| width/height | `.w(f)` `.h(f)` `.size(w,h)` `.w_full()` `.h_full()` `.w_frac(0.4)` `.min_w/.max_w/.min_h/.max_h(f)` |
| flex | `.grow(g)` (= CSS `flex: g 1 0`), `.shrink()` (allow shrink below content) |
| spacing | `.gap(f)` `.padding(f)` `.px(f)` `.py(f)` `.pt/.pb/.pl/.pr(f)` `.mt/.mb/.ml/.mr/.mx/.my(f)` |
| alignment | `.center()` (both axes) `.center_y()`/`.items_center()` `.items_start()` `.items_end()` `.justify_between()` `.justify_end()` `.justify_center()` `.flex_wrap()` |
| fill / radius | `.fill(Color)` `.radius(f)` `.corners([tl,tr,br,bl])` `.continuous_corners()` |
| border | `.border(width, Color)` (inside the box, takes no layout space) `.outline(w, offset, Color)` `.border_dash(d,g)` |
| shadow/gradient | `.shadow(y, blur, Color)` `.box_shadow(Shadow{..})` `.gradient(top, bottom)` `.linear_gradient(angle, &[(t, Color)])` `.radial_gradient(..)` |
| opacity/blur | `.opacity(f)` `.blur(sigma)` `.backdrop_blur(..)` (see README "Backdrop blur") |
| clip | `.clip_rounded(radius)` (children clipped to rounded rect) `.clip_corners([..])` `.clip_soft(fade)` (soft edge, never a hard crop) |
| absolute | `.absolute(left, top)` or `.pin(Some(l), Some(t), Some(r), Some(b))` (both opposite edges = stretch); relative to parent's padding box. Every element is a containing block |
| hover/press | `.hover(Color)` / `.press(Color)` = **overlay drawn over the fill** (use translucent colours, e.g. white at 0.06 alpha); `.press_scale(0.97)`; group hover: parent `.hover_group()`, child `.group_opacity(rest, hovered)` / `.group_color(c)` |
| cursor | `.cursor(Cursor::Pointer)` (also `Text, Grab, Grabbing, NotAllowed, Move, ResizeColumn, ResizeRow, Crosshair, Hidden, Image(..)`); inherited by children |
| identity/a11y | `.key("id")` (unique among siblings) `.role("button")` `.label("Play")` `.focusable(true)` |
| text | `text(s).font(TextStyle::new(size, weight)).color(Color)`; `.truncate()` `.wrap()` `.max_lines(n)` `.truncate_fade()` `.align(Align::..)` |

Text defaults: `text()` is **white**, 13 pt / 400 (`tok::type_::BODY`). `TextStyle::new(size: f32, weight: f32)` is
`const`; `.lh(line_height)`, `.tracking(f)`, `.features("'tnum' 1")` (tabular figures for time labels), `.italic()`,
`.mono()`, `.family(Family::Named("X"))`.

### Colours (hex)
`rune::Color` is **linear sRGB** `{r,g,b,a}` floats, so do not build it from 0..1 hex bytes by hand. Use
`rune::color::hex("#ff7a8a") -> Option<Color>` (accepts `#rgb`, `#rrggbb`, `#rrggbbaa`, sRGB, converts to linear).
Also `rune::oklch(l,c,h)`, `Color::WHITE/BLACK/TRANSPARENT`, `c.alpha(0.5)`, `c.mix(other, t)`.
```rust
use std::sync::LazyLock;
use rune::Color;
fn hx(s: &str) -> Color { rune::color::hex(s).expect("bad hex") }
struct Pal { bg: Color, panel: Color, text: Color, muted: Color, accent: Color }
static PAL: LazyLock<Pal> = LazyLock::new(|| Pal {
    bg: hx("#0e0e10"), panel: hx("#17171a"), text: hx("#f2f2f3"), muted: hx("#9a9aa2"), accent: hx("#ff7a8a"),
});
// el().fill(PAL.panel).hover(Color::WHITE.alpha(0.06)).radius(8.0)
```
Built-in token set if wanted: `rune::tok::theme(dark) -> tok::Theme` (`bg, card, text, text_secondary, separator, hover, ...`),
`tok::accent(i, dark)`, `tok::space::{S1,S2,S3,S6}`, `tok::radius::{SM,MD,LG}`, `tok::type_::{BODY,LABEL,SECTION,TITLE}`.
`Theme::from_json` exists for named tokens as data (hex/rgba), not needed for us.

### Fonts
- Native builds **always bundle Inter 4.1** (Regular 400, Medium 500, SemiBold 600 only; `crates/rune/assets/`) and the
  default `families` is `("Inter", "Inter")`. So Inter works with no setup. **No Bold (700) face and no italic face
  are bundled**; how 700 resolves (nearest face vs synthesis) is unverified, so stay on 400/500/600.
- **System fonts are not scanned for the UI font** (the collection is created with `system_fonts: false`). Only
  `Family::SystemMono` (fontconfig `monospace`), CJK fallbacks (found via `fc-list`) and symbol/emoji fallbacks
  come from the system. So "Inter if installed" is not a thing: Inter is always the embedded one.
- Bundle other/full fonts: `AppSpec { fonts: &[include_bytes!("../assets/Inter-Variable.ttf")], families: ("Inter", "Inter"), .. }`.
  A non-empty `fonts` **replaces** the built-in Inter. `families` takes CSS-style lists. Variable axes via
  `TextStyle::variations("'wght' 700")` (CSS syntax). Runtime swap: `tx.set_font_families(sans, mono)`.
- Text AA default on Linux is `FontSmoothing::Off` (greyscale, lighter). `App::font_smoothing(FontSmoothing::On)` thickens.

---------------------------------------------------------------------------------------------------

## 3. Scrolling and long lists

```rust
use rune::{ScrollSpec, GrowthSpec, col};
// Content area. The viewport needs a BOUNDED height: a bounded ancestor + grow(1) + min_h(0) on every
// flex column between that ancestor and the viewport.
col().w_full().h_full()
    .child(top_bar)
    .child(col().grow(1.0).min_h(0.0)
        .child(col().key("content").scroll(ScrollSpec { stick_bottom: false }).grow(1.0)
            .child(page_content)))
    .child(player_bar)
```
- `.scroll(ScrollSpec { stick_bottom })` vertical; `.scroll_x()` horizontal (a shelf of cards; content gets natural
  width); `.scroll_xy()`. Wheel/trackpad/fling momentum are built in. `.scroll_edge_fade(w)`, `.sticky_top(inset)` (section
  headers), `.scroll_anchor()` (keeps reading position when content above changes).
- Programmatic: `tx.scroll_to(key, x, y)` (glide), `tx.scroll_to_instant(..)`, `tx.scroll_into_view(row_key)`.
  Reset a page's scroll on navigation by giving the scroll element a different `.key` per page (not verified, but
  scroll offsets are runtime-owned by element identity).
- **Virtual lists** (vertical only; rows built lazily by index):
```rust
col().key("songs").growth(GrowthSpec::default())                 // required on the same element
    .virtual_rows(
        s.songs.len(), 56.0,                                      // count, estimated row height
        |s: &Ui, i| s.songs[i].id.as_str().into(),                // unique stable key per row -> Cow<str>
        |s: &Ui, i| {
            let song = &s.songs[i]; let id = song.id.clone();
            row().w_full().px(16.0).py(8.0)                       // row spacing = INTERNAL padding, not margins/gap
                .on_press(move |s: &mut Ui| s.play_song(&id))
                .child(text(song.title.clone()))
        })
```
  The element must be the **first/direct child of the `.scroll(..)` viewport** and hold no other children. Rows need
  unique keys and vertical padding rather than margins or gaps. Rune builds only rows near the viewport (viewport +
  2 either side); `memo(key, version, || row)` caches row subtrees. `.on_visible_rows(|s, range| bool)` is the hook for
  paging in more data. `.virtual_rows_version(v)` skips the per-row `key` call for huge lists.
- There is **no virtualized grid**. For album grids, make each virtual row a `row()` of N cards (chunk the data) or
  use a plain scroll column with `.flex_wrap()` for modest counts. Non-virtual `.virtualize(est)` exists too (children
  pre-built; "view construction still visits every row").

---------------------------------------------------------------------------------------------------

## 4. Text input (search box)

`TextEdit` lives in state. `text_input` is **multi-line by default** and has no background/padding of its own.
```rust
use rune::{TextEdit, TextStyle, Tx, text_input, row, icon};
struct Ui { query: TextEdit, /* .. */ }          // TextEdit::new("") ; value = s.query.text (pub String)

fn search_box(s: &Ui) -> El {
    row().key("search-box").w(280.0).h(34.0).px(10.0).gap(8.0).center_y().radius(8.0).fill(PAL.panel)
        .child(icon(&rune_kit::glyphs::SEARCH, 14.0).color(PAL.muted))   // or our own Icon (section 7)
        .child(text_input(&s.query, TextStyle::new(13.0, 400.0))
            .key("search").grow(1.0)
            .single_line()                                     // after text_input(); one line, scrolls sideways
            .color(PAL.text)
            .placeholder("Search", PAL.muted)                  // (text, Color)
            .on_edit(|s: &mut Ui| &mut s.query)                // binds field <-> state
            .on_input_change(|s: &mut Ui, _tx: &mut Tx| { /* every edit; e.g. debounce a live search */ })
            .on_submit(|s: &mut Ui, tx: &mut Tx| {             // Enter
                let q = s.query.text.trim().to_owned();
                if !q.is_empty() { s.send_search(q); }
                tx.blur();                                      // optional: return keys to window on_key
            }))
}
```
- Focus: set `edit.focused = true` once to request focus (runtime clears it; do not force it every view build).
  `edit.clear()` / `edit.replace_text(..)` for programmatic changes (not direct assignment to `.text` if you care about
  history/secure wiping). `.on_input_key(..)` on the field intercepts keys before editing.
- Editing keys (caret, selection, copy/paste, undo) and IME are built in. Press-anywhere-focus: put `.role("textbox")`
  on the wrapper and a press on it focuses the single editor inside.
- **rune-kit alternative:** `TextInput::new(&s.query).placeholder("Search").leading_icon(&rune_kit::glyphs::SEARCH)
  .clear_button().on_edit(|s: &mut Ui| &mut s.query).on_submit(|s: &mut Ui, tx| ..).build(&kit)`.
- While a text field has focus, **Space and letters go to the field, not to `AppSpec::on_key`** (section 8).
- Requests per keystroke: use `rune::Latest` (section 6), never block in a handler (debug builds warn at 8 ms).

---------------------------------------------------------------------------------------------------

## 5. Images (artwork from downloaded bytes)

**Rune has no HTTP client** (native `ImageCache::url` only reads paths/`file:` URLs). We fetch bytes ourselves (any
HTTP crate, on a worker thread) and hand them over. API: `rune::ImageCache` -> `rune::Image` -> `El::pixels`.
```rust
use std::collections::HashMap;
use rune::{ImageCache, Image, SurfaceFit, Waker, el};

struct Ui { cache: Option<ImageCache>, art: HashMap<String, Image> /* KEEP handles here */, .. }

fn setup(s: &mut Ui, wake: Waker) { s.cache = Some(ImageCache::new(wake)); }   // AppSpec.setup; spawns decode workers

// on the UI thread (e.g. inside handle.update, section 6) when downloaded bytes arrive:
fn got_art(s: &mut Ui, url: String, bytes: Vec<u8>) {
    let img = s.cache.as_ref().unwrap().load(bytes);     // impl Into<Arc<[u8]>>; PNG/JPEG decoded off-thread
    s.art.insert(url, img);                              // decode finishes later -> waker -> redraw
}

fn cover(s: &Ui, url: &str, size: f32) -> El {
    let b = el().size(size, size).radius(8.0).fill(PAL.panel);       // placeholder fill while pending
    match s.art.get(url) {
        Some(img) => b.pixels_fit(img.pixels(), SurfaceFit::Fill),   // Fill = cover + crop; Fit = letterbox
        None => b,
    }
}
```
- `cache.load(bytes)` accepts PNG, JPEG (EXIF-oriented), PNG-backed ICNS, SVG; **WebP/AVIF are not decoded natively**
  (cargo features `png`, `jpeg` only), so request `.jpg`/`.png` artwork URLs from Apple. Max side 8192.
  `cache.rgba(w, h, straight_alpha_rgba8_bytes)` takes already-decoded pixels.
- **Rounded corners:** `.radius(r)` on the element rounds the image (`pixels` is "clipped to its corner radius");
  `.corners([..])` per corner; circle = `radius(size/2)`.
- Sizing: an element with only `.w(..)` takes height from the image ratio; `.aspect_ratio(1.0)` forces a ratio.
- **Caching:** `ImageCache` dedupes by blake3 of the bytes and holds entries **weakly**: decoded pixels live only
  while some `Image`/`Pixels` clone exists. You must keep `Image` handles in state (or an LRU) or they are freed and
  re-decoded. Cost is ~4 bytes/pixel plus mips (~1.33x), so request artwork at display size x2 (e.g. 600x600 grid,
  not 3000x3000) and cap the map (LRU of a few hundred). Keep your own byte/disk cache for HTTP. Equal bytes -> same
  handle, so duplicate URLs are free. `Image::{is_ready, error, size, wait}`; `wait()` blocks (only for headless fixtures).
- `ImageCache` has an `mpsc::Sender` + `Mutex` inside, so it should be usable behind an `Arc` from other threads
  (auto-trait impls not confirmed by compiling).
- `rune::Pixels` (+ `Pixels::set(w, h, rgba)` from any thread) is the other route for a frequently replaced image.

---------------------------------------------------------------------------------------------------

## 6. Background threads -> UI (most important)

Ground truth: state `S` lives on the UI thread. Other threads reach it in one of three verified ways.

**(A) Recommended: `AppHandle::update` (a `Send + Sync + Clone` handle).** `App::new(spec).handle()` returns an
`AppHandle<S>`; `handle.update(|s: &mut S, tx: &mut Tx| ..)` queues a closure that runs **on the UI thread**, in
call order, wakes the loop itself, and the next frame commits. Works before `run` (queued until start), while the
window is closed, and in headless runs (applied before each frame). `handle.update_if(|s, tx| -> bool)` skips the
redraw when you return false (e.g. a position tick that did not change anything visible).

**(B) `setup` Waker + your own channel + `poll`.** `setup` gets a `Waker` (`Arc<dyn Fn()+Send+Sync>`); call it from any
thread after pushing to a channel/queue; `poll` runs, drains, returns true if state changed. Because `init` is a `fn()`,
get a receiver into `S` with `App::init(move || Ui::new(rx))`.

**(C) `rune::Latest<T>`** for "one request per keystroke, only the newest answer matters" (search-as-you-type):
```rust
// state: hits: Vec<Hit>, results: Latest<Vec<Hit>>   init: Latest::default().debounce(Duration::from_millis(60))
setup: Some(|s: &mut Ui, w| s.results.set_waker(w)),
// handler (each keystroke):  s.results.request(q.clone(), |q| blocking_http_search(&q));
// poll: let Some(h) = s.results.take() else { return false }; s.hits = h; tx.instant(); true
```
Latest runs the closure on a shared pool (max 4 threads), drops stale answers, `is_pending()` for spinners,
`request_if_changed(&key, ..)`, `cancel()` (call from `App::on_close`). It is not for event streams like the bridge.

### Complete minimal pattern (bridge reader thread + HTTP fetch threads), pattern A
```rust
use std::{io::{BufRead, BufReader, Write}, process::{Command, Stdio}, sync::mpsc, thread};
use rune::{App, AppHandle, AppSpec, ImageCache, Image, Waker};

enum Event { Playback { playing: bool, pos: f64, dur: f64 }, Results(Vec<Hit>) }   // parsed from JSON lines

struct Ui {
    cmd: mpsc::Sender<String>,                  // UI -> Electron: JSON lines (a writer thread owns the Receiver)
    handle: Option<AppHandle<Ui>>,              // so handlers can spawn fetch threads that report back
    cache: Option<ImageCache>, art: std::collections::HashMap<String, Image>,
    playing: bool, pos: f64, dur: f64, hits: Vec<Hit>,
}
impl Ui {
    fn apply(&mut self, e: Event) -> bool {
        match e {
            Event::Playback { playing, pos, dur } => { let changed = self.playing != playing || self.dur != dur
                || (self.pos - pos).abs() >= 0.25; (self.playing, self.pos, self.dur) = (playing, pos, dur); changed }
            Event::Results(h) => { self.hits = h; true }
        }
    }
    fn fetch_art(&self, url: String) {
        let Some(h) = self.handle.clone() else { return };
        thread::spawn(move || {
            let Some(bytes) = http_get(&url) else { return };        // our HTTP client of choice (ureq/reqwest)
            h.update(move |s: &mut Ui, tx| { tx.instant(); let img = s.cache.as_ref().unwrap().load(bytes); s.art.insert(url, img); });
        });
    }
}

fn spawn_bridge(handle: AppHandle<Ui>, cmd_rx: mpsc::Receiver<String>) {
    let mut child = Command::new("electron-bridge-binary").stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().expect("spawn");
    let (mut stdin, stdout) = (child.stdin.take().unwrap(), child.stdout.take().unwrap());
    thread::spawn(move || for line in cmd_rx { if writeln!(stdin, "{line}").and_then(|_| stdin.flush()).is_err() { break } });
    thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if let Some(ev) = parse_event(&line) {                      // serde_json
                handle.update_if(move |s: &mut Ui, _tx| s.apply(ev));   // runs on the UI thread; wakes it
            }
        }
        handle.quit();                                                  // bridge died -> close app (or show an error state)
    });
}

fn main() {
    let (cmd_tx, cmd_rx) = mpsc::channel();
    let app = App::new(AppSpec::<Ui> {
        setup: Some(|s: &mut Ui, w: Waker| { if s.cache.is_none() { s.cache = Some(ImageCache::new(w)); } }),
        ..AppSpec::new("Cadence", (1180.0, 760.0), Ui::fake, view)   // Ui::fake is the init fn pointer (used by headless/scenes)
    })
    .init(move || Ui::real(cmd_tx));                                 // closure may capture the Sender; overrides spec.init
    let handle = app.handle();
    if !rune::is_headless() {                                        // never spawn Electron for --snap/--tree
        // spawn from on_ready if startup latency matters: app.on_ready(move |_| ..)
        spawn_bridge(handle.clone(), cmd_rx);
        let h2 = handle.clone(); handle.update(move |s: &mut Ui, _| s.handle = Some(h2));   // queued before run
        // Ui::fake / Ui::real are your constructors (fake data vs real channel)
    }
    app.run();                                                       // returns when the window closes
}
```
Notes:
- Do all parsing on the reader thread; the closure passed to `update` should only mutate state.
- `update` closures are `FnOnce + Send + 'static`: move owned data in (the `Event`, `bytes`).
- Throttle high-rate events (time ticks) on the reader thread or with `update_if` returning false; every committed
  update re-runs `view`. A progress bar at 4 Hz is fine as a normal commit. Alternatives that avoid commits:
  `rune::LiveText` for a clock label and `rune::Live<[f32;4]>` + `.live_rect(&live)` for a bar fill (see README
  "High-frequency values"; not needed to start).
- Scene fns (`--scene`) run **before** `setup`, so `cache` is `None` there; guard accordingly (section 10).
- The reader thread outliving `run`: after `app.run()` returns, kill the child process (keep it in an `Arc<Mutex<Child>>`).
- Sending commands from handlers: `s.cmd.send(json_line).ok()` (non-blocking; writer thread does the IO).
- `AppHandle` also has `wake()` (runs `poll`), `open()`, `close()`, `minimize()`, `quit()`, `is_visible()`.

---------------------------------------------------------------------------------------------------

## 7. Sliders, progress, icons

**No slider component in rune-kit** (kit has `Progress`, `ProgressCircle`, `SegmentedControl`, `Switch`; all non-draggable
or non-slider). Rune's own `examples/settings` hand-rolls a slider with `.on_drag`; `rune::Drag` gives the pointer in the
element's own points plus its size, so a flexible-width seek bar needs no fixed width:
```rust
use rune::{Cursor, Drag, Phase, Tx, el, row};
fn seek_bar(frac_now: f32, accent: Color, track: Color) -> El {
    el().key("seek").role("slider").label("Seek").w_full().h(16.0).center_y().cursor(Cursor::Pointer)
        .on_step(|s: &mut Ui, steps, _tx| s.seek_by(steps as f64 * 5.0))        // keyboard: arrows/PageUp/PageDown when focused
        .on_drag(|s: &mut Ui, d: &mut Drag, tx: &mut Tx| {
            let f = (d.x / d.w).clamp(0.0, 1.0);                                // d.x, d.w: element-local points
            match d.phase {
                Phase::Down => s.scrub = Some(f),
                Phase::Move => { s.scrub = Some(f); tx.instant(); }
                Phase::Up => { if !d.cancelled() { s.seek_to_fraction(f); } s.scrub = None; }  // send seek to Electron on release
            }
        })
        .child(row().w_full().h(4.0).radius(2.0).clip_rounded(2.0).fill(track)
            .child(el().grow(frac_now).fill(accent))                            // flex-basis 0: widths split in ratio
            .child(el().grow((1.0 - frac_now).max(0.0))))
}
// view passes frac_now = s.scrub.unwrap_or(pos/dur) so the bar follows the pointer while dragging.
```
Also `.range_value(v, min, max)`, `.range_step(f)`, `.a11y_value(..)` for accessibility. Volume slider: same pattern.
(The settings example draws a knob with `.absolute(x, y)`; the flex-ratio fill above is my variant, not copied from there.)

**Icons: no built-in icon set suitable for transport controls.** `rune::icon(&'static Icon, size) -> El` draws a small
SVG subset (`path`, `rect`, `circle`, `ellipse`, `line`; fill/stroke, caps, joins, evenodd, opacity) rasterised into the glyph
atlas and tinted with `.color(c)`. `Icon { name, view, body }` are pub fields: define your own as statics. The kit ships
a few glyphs (`rune_kit::glyphs::{SEARCH, CHECK, CHEVRON_DOWN, CLOSE, COPY, STAR, STAR_FILLED, FOLDER, ..}`), and
`themes/opencode/src/icons.rs` has ~170 (menu, plus, chevrons, arrows; no play/pause/skip/volume that I found).
```rust
use rune::{Icon, icon, el};
pub static PLAY: Icon  = Icon { name: "cad-play",  view: 24.0, body: r#"<path d="M7 4.5v15l12.5-7.5z" fill="currentColor"/>"# };
pub static PAUSE: Icon = Icon { name: "cad-pause", view: 24.0, body: r#"<rect x="6" y="5" width="4" height="14" rx="1" fill="currentColor"/><rect x="14" y="5" width="4" height="14" rx="1" fill="currentColor"/>"# };
pub static NEXT: Icon  = Icon { name: "cad-next",  view: 24.0, body: r#"<path d="M5 5l10 7-10 7z" fill="currentColor"/><rect x="16.5" y="5" width="2.5" height="14" rx="1" fill="currentColor"/>"# };
fn round_btn(ic: &'static Icon, label: &'static str, f: impl Fn(&mut Ui) + 'static) -> El {
    el().key(label).role("button").label(label).focusable(true).size(40.0, 40.0).radius(20.0).center()
        .hover(Color::WHITE.alpha(0.08)).press(Color::WHITE.alpha(0.14)).cursor(Cursor::Pointer)
        .on_press(f).child(icon(ic, 22.0).color(PAL.text))
}
```
Keep `name` unique per icon (it is the identity shown in `Icon`; atlas keying by name is plausible but unverified).
Alternatives: `canvas(Shape)` / `.shape(Shape)` vector paths (`rune::{Shape, Path, LineCap}`), or `ImageCache::svg(bytes, size_pt, scale)`
for an SVG file (needs the `svg` feature, on by default). kit `IconButton::new(&ICON).label("Play").on_press(..).build(&kit)`
gives a themed icon button with focus/hover states.

---------------------------------------------------------------------------------------------------

## 8. Keyboard shortcuts

`AppSpec::on_key: fn(&mut S, &KeyEvent, &mut Tx) -> bool`. `KeyEvent { key: Key, mods: Mods }`,
`Mods { shift, cmd, alt, ctrl, logo }` (`cmd` = Ctrl on Linux).
```rust
use rune::{Key, KeyEvent, Tx};
fn on_key(s: &mut Ui, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if ev.mods.cmd || ev.mods.alt { /* chords below */ } 
    match &ev.key {
        Key::Text(t) if t == " " && !ev.mods.cmd => { s.toggle_play(); true }   // plain Space = Key::Text(" ")
        Key::Command('f') => { s.focus_search = true; true }                     // Ctrl+F (lowercased char)
        Key::Command('q') => { tx.quit(); true }
        Key::Right if ev.mods.alt => { s.next(); true }
        Key::Escape => { s.close_now_playing(); true }
        _ => false,
    }
}
```
- Order: nothing focused -> `on_key` first; focused field's `on_input_key` -> focus navigation (Tab, arrows in groups) ->
  field editor -> `on_key` again for keys the control passed on. **A focused text field eats Space/letters**; a
  focused button takes Space (presses it). `tx.focused()` tells which case. Call `tx.blur()` after search submit
  so Space toggles playback again. **A clicked button keeps the focus**, so the next Space would press it again
  (Shuffle shuffles again): an `El::on_key` on an ancestor sees the key first whenever the focus is not a text field
  (`element_key`, before `focus_key` presses), so Uzumaki's window root runs the play/pause chord there
  (`keys::control_key`). Space never presses buttons, rows or menu items; Enter still picks rows and highlighted menu items.
- **Focus origin (`:focus-visible`)**: Rune keeps `keyboard_focus` (set by Tab and the arrows, cleared by any pointer
  press) and draws its focus ring only then (`Cx::focus_ring(w, offset, color)` sets the default ring; Uzumaki: 2 pt coral,
  2 pt out), but **handlers can't read it** and there is no focus-change or global pointer-down hook. Return presses only
  rows, links and the `default_button`, never a plain button; Space presses any focused control. So Uzumaki tracks it
  itself (`keys::KbdFocus`): a Tab (seen by the root `on_key`, the app `on_key` when nothing is focused, or a field's
  `on_input_key`) sends a marker key (`tx.send_key(F24 + every modifier)`, queued after the Tab, so it arrives with
  `tx.focused()` = where Tab landed); Enter on that element sends a plain Space marked to get past play/pause, which
  Rune's `focus_key` turns into the press (and takes even with no `on_press`). Clicks: the root's
  `on_pointer_move_local` (+ `tx.live_only()`, no view run) sees every release as a "move" to the point the pointer
  already was, which forgets the record. Esc with nothing to close blurs. `App::tab_visits_controls(true)`: without it
  macOS Tab visits only fields and lists unless the user turned on Keyboard navigation.
- Portable chords: `Chord::parse("mod+shift+t")?`, `chord.matches_tx(ev, tx)`; `mod` = Ctrl on Linux.
- Per-element: `El::on_key(|s, ev, tx| bool)`, `on_key_up`. Global (system-wide) shortcuts exist
  (`App::global_shortcut` / `register_shortcut`; X11 and the Wayland GlobalShortcuts portal) but whether XF86Audio media
  keys can be expressed as a `Chord` is unverified: plan on MPRIS for media keys (not part of Rune).

---------------------------------------------------------------------------------------------------

## 9. Window/app lifetime, tray

- `rune::run` / default `App`: **closing the window quits** the process (`run` returns).
- `App::keep_running()` keeps process, state and loop after close (Wayland destroys the window, X11 hides it);
  `App::windowless()` starts hidden; `AppHandle::open()/activate(token)/close()/quit()` control it from any thread.
  `App::on_close(fn(&mut S, &mut Tx))` runs before every close (call `tx.quit()` to quit instead). Tx: `tx.close_window()`,
  `tx.quit()`, `tx.minimize()`. Hidden/covered windows still run updates and `poll` but skip view/layout (cheap).
- `rune::set_app_id("dev.cadence.app")` before `run` sets Wayland `app_id`/X11 `WM_CLASS` (matches `.desktop`, icon).
  Linux windows get the native frame (Wayland CSD adwaita). `El::window_drag()` makes a custom title area draggable.
- **Tray exists** (`rune::Tray`, `rune::tray::{TraySpec, TrayIcon, TrayItem}`, `TrayEvent::{Activate, Menu(id)}`): StatusNotifierItem
  + dbusmenu over zbus on Linux, needs the default `dbus` feature, runs on its own thread (callback must be `Send + Sync`
  and short: forward via `handle.update`), shows only where a tray host runs (GNOME needs an extension). Combine with
  `keep_running()` for close-to-tray. We are not building it now.
- No MPRIS in Rune. Rune's `dbus` feature uses zbus internally for tray/portals, which does not give us an MPRIS service.

---------------------------------------------------------------------------------------------------

## 10. Headless checking (no display, no network)

Any app using `rune::run` or `App::run` accepts these flags automatically (parsed by `rune::tooling::Args`), and renders
offscreen with the same renderer on a fixed virtual clock:
```sh
BIN=target/release/cadence
$BIN --list-scenes                                   # prints the names in AppSpec::scenes
$BIN --snap /tmp/album.png --scene album --size 1180x760 --scale 1     # PNG; --scale default 2
$BIN --snap /tmp/a.png --scene album --tree /tmp/a.json                # element tree JSON (also usable alone: --tree FILE)
$BIN --snap /tmp/np.png --scene now-playing --size 1180x760 --theme dark [--at 0.5]
$BIN --script steps.json --snap out.png --at mark:end                  # scripted input (clicks, keys, typing, scroll), then snapshot
$BIN --script steps.json --frames 30 --out frames/                     # PNG frames
```
Flags: `--snap FILE`, `--scene NAME` (dies listing valid names if unknown), `--size WxH` (points), `--scale N`,
`--theme dark|light`, `--at SECONDS|mark:NAME`, `--tree FILE`, `--script FILE`, `--frames N`, `--out DIR`, `--video FILE`
(needs ffmpeg), `--crop KEY` (with `--snap`: crop to a keyed element), `--scroll-bars always|when-scrolling|automatic`, `--help`.
`--tree` fields per element: `key`, `path`, `role`, `label`, `text`, presented rect, target, opacity, fill/text colour, hover/press, moving.
Unknown args are left in `rune::extra_args()` for our own flags.

Scenes are a static table of **non-capturing** `fn(&mut S)` that tweak the initial state (they run after `init`/`App::init`
and **before `setup`**):
```rust
fn scene_album(s: &mut Ui) { s.page = Page::Album("fake"); s.album = fake_album(); s.playing = true; s.pos = 83.0; s.dur = 241.0; }
fn scene_now_playing(s: &mut Ui) { scene_album(s); s.now_playing_open = true; }
AppSpec { scenes: &[("album", scene_album), ("now-playing", scene_now_playing)], .. }
```
- In headless runs `rune::is_headless()` is true (skip spawning Electron, tray, network). `setup` still runs (Waker is a no-op).
- Fake artwork: images decode asynchronously, so a `--snap` can be taken before they land. For layout checks draw
  deterministic placeholder fills; if you need real pixels in a scene, create `ImageCache::new(Arc::new(|| {}))` in the scene
  fn, `cache.rgba(w, h, bytes)` and call `img.wait()` before returning (and keep `setup` from overwriting it, as in section 6).
- Scripts (JSON object `{"events": [..]}`, see `examples/settings/scripts/toggle.json`): `{"t":0.1,"type":"tap","key":"search"}`, `{"type":"type","text":"hello"}`,
  `{"type":"key","name":"Enter"}`, `{"type":"scroll","dy":-300}`, `{"type":"drag","key":"seek","dx":120,"dy":0,"dur":0.3}`,
  `{"type":"mark","name":"end"}`. Keys/path address elements by `.key`, so key everything you want to click.
- Needs a Vulkan adapter even headless (a software one works, see section 11); no display server or XKB-x11 needed for `--snap`.
- Unit-test route: `Runtime::new(state, view, (w,h), Env{system_dark:true, scale:1.0})`, `rt.frame(dt)`, `rt.find("key")`,
  `rt.pointer_down/up(x,y)`, `rt.presented("key")` (see `examples/hello/src/main.rs` tests).

---------------------------------------------------------------------------------------------------

## 11. Build requirements, Cargo, toolchain

```toml
[dependencies]
rune = { git = "https://github.com/anomalyco/rune", rev = "f3d04f3f2069d177d04c963b0ba9f6b2065e46eb" }
rune-kit = { git = "https://github.com/anomalyco/rune", rev = "f3d04f3f2069d177d04c963b0ba9f6b2065e46eb" }   # optional
serde_json = "1"   # ours, for the bridge protocol
```
(`rune` and `rune-kit` live under `crates/` in a workspace; cargo resolves git deps by package name.) The Rune repo is
edition 2024 with `rust-toolchain.toml` `channel = "1.99.1"` (+ clippy, rustfmt) which matches our rustc 1.99.1. The pin file
only affects builds *inside* the Rune checkout; our crate uses our own toolchain.

- **System packages (Arch):** build: `pacman -S libxkbcommon wayland` (Rune README), plus `pkgconf`/base-devel. Runtime:
  a Vulkan driver for your GPU (wgpu uses Vulkan on Linux: `vulkan-icd-loader` + `vulkan-radeon`/`vulkan-intel`/`nvidia-utils`;
  software fallback `vulkan-swrast` for headless/CI). Windowed runs on X11 need `libxkbcommon-x11` (README: without it
  startup panics "libxkbcommon-x11.so could not be loaded"; Arch package names from memory). X11 and Wayland client libs are
  `dlopen`ed. Optional: `fontconfig` (`fc-list`/`fc-match` used for CJK/mono fallbacks), `ffmpeg` for `--video`.
- **Features** of `rune` (default = `accessibility, dbus, embedded-fonts, metal-backend, ogg-audio, svg`; the Apple-only ones
  are no-ops on Linux). Keep defaults: `dbus` = tray + system dark/light + portals; `svg` = SVG icons/images; `accessibility` = AT-SPI.
  `native-file-dialog` (XDG portal file dialogs), `hot` (live patching), `frame-profile` are off and not needed. Trimming defaults
  (`default-features = false, features = ["dbus","svg"]`) is plausible but unverified. `rune-kit` features: `markdown`, `mermaid`.
- First build compiles wgpu, winit, taffy, parley, resvg, zbus, tokio: expect a long cold build; use `[profile.dev.package."*"] opt-level = 2`
  (Rune's own workspace sets `debug = false` for deps) if dev frame times are bad. Debug builds add extra perf warnings
  (`RUNE_PERF_WARN=0` silences; `RUNE_COMMIT_BUDGET_MS=8` reports slow commits in any build).
- Rune is pre-release: `dev` is fast-forward-only and apps pin revs; no semver guarantees.

---------------------------------------------------------------------------------------------------

## 12. rune-kit components (optional; needs a `Kit`)

`let k = rune_kit::Kit::neutral(dark)` or themed from our palette:
`Kit::from_palette(PaletteSeed { bg, fg, accent })` (derives layers, borders, hover, focus from three colours; set
`k.radius.sm = ..`, `k.c.border_focus`, etc.). Builders end with `.build(&k)`; overlays (Select, Menu, Popover, Tooltip)
take `.build(&k, &mut layer)` and the view root must be `rune_kit::root(body, layer)`. Handlers are set on the builder.
- Buttons: `Button::new(label)` (10 variants, 3 sizes, `.icon(&ICON)`, `.on_press`), `IconButton::new(&ICON)` (`.label`, `.help`).
- Inputs: `TextInput`, `TextArea`, `Field`, `Combobox` (filterable popup), `Select`, `Switch`, `Checkbox`, `RadioGroup`, `SegmentedControl::new(&labels, Some(i))`.
- Nav/shell: `SplitView` + `Sidebar`/`SidebarRow`/`SidebarSection` (resizable collapsible sidebar, arrow-key list, virtual rows),
  `Toolbar`, `Page`, `TabStrip` (generic tab strip, reorder/tear-out; heavier than we need), `PaneState`.
  The shell is built around macOS-style titlebar chrome; for our mockup a hand-built `col()` sidebar is simpler.
- Lists/tables: `List`/`ListItem`, `Table` (virtualised, sortable, resizable columns), `Tree`, `PopupList`, `Accordion`, `Collapsible`.
- Menus/overlays: `Menu`/`MenuItem`/`context_menu(..)`, `Popover`, `Tooltip`, `HoverCard`, `Dialog`, `Toast`/`ToastRegion`.
- Feedback: `Progress::new(Some(0.4))` (display only), `ProgressCircle`, `Spinner`, `Loader`, `Badge`, `Divider`, `Card`, `Keybind`, `ScrollBar`.
- Text: `RollingText`, `Markdown` (feature), `diff`, `syntax`.
- **Missing:** Slider, a plain Tabs component (use `SegmentedControl` or hand-roll), icon set, grid.
- Kit components follow OpenCode Desktop's look; to match the Cadence mockup (coral accent, Inter, dark) either theme through
  `PaletteSeed` or hand-style with `el()` + our palette. Plain `el()` is the path of least friction for the player bar and cards.
- rune-kit depends on `rune` with `default-features = false`; cargo unifies features, so ours (defaults) still apply.

---------------------------------------------------------------------------------------------------

## 13. Gotchas (docs and source)

1. **`poll` / `update_if` return true only when visible state changed**, else an idle window redraws constantly. Draw nothing when idle.
2. **`AppSpec` fields are `fn` pointers**; no captures. Capture via `App::init(closure)`, `AppHandle`, or globals. Build it as
   `AppSpec { .., ..AppSpec::new(..) }`.
3. The `view` re-runs on every committed state change and builds the **whole tree**; keep it cheap, use `virtual_rows` for long
   lists and `memo(key, version, || el)` for rows that rarely change. Never block (network, disk, JSON) in view, handler or poll
   (debug builds warn at 8 ms).
4. **Scroll viewports need a bounded height**: `.grow(1.0)` is `flex: g 1 0` (zero basis), so in an auto-height column it collapses to 0.
   Bound an ancestor (`.h_full()`), and put `.min_h(0.0)` on every flex column between it and the viewport, or the content never scrolls.
   Debug builds warn once per call site.
5. `.virtualize`/`.virtual_rows` need `.growth(GrowthSpec::default())` on the same element, as the scroll viewport's first child;
   missing growth panics with the element path. Rows: unique keys, internal vertical padding (no gap/margin). Handlers in rows must
   capture the item id, not the index.
6. **Keys:** siblings need unique `.key`s (duplicates become `key#dup1` with a warning). Keyed nodes retain layout caches, keys identify
   elements for the diff/animation, scripts, `--crop`, `tx.scroll_*`. rune-kit keys its parts (`label`, `value`, `icon`, ...), so scope
   your own with a prefix or path (`"album/title"`).
7. Animation: layout/colour changes animate on springs by default; the first frame never animates. Use `tx.instant()` when data lands
   (artwork, search results, ticks) so lists don't swim; drag moves are instant by default.
8. Paint hover/press with `.hover()/.press()/hover_group`, never with state set from `on_hover_change` (re-runs the view for each row
   crossed while scrolling).
9. `Color` is linear; use `rune::color::hex`. `text()` defaults to white. `.hover()/.press()` are overlays over the fill.
10. `el()` is a **row**. `text_input` is **multi-line** by default; add `.single_line()` and give it a styled wrapper.
11. `.border()` takes no layout space and replaces the fill beneath it (translucent border shows the backdrop).
12. Images: keep `Image` handles alive (cache is weak), request sensible sizes, PNG/JPEG only, Rune cannot fetch URLs.
13. Fonts: only Inter 400/500/600 bundled; system UI fonts are not used; custom fonts via `AppSpec::fonts` replace Inter.
14. `on_key` does not see keys a focused text field consumed (Space, letters); blur after submit. Plain Space is `Key::Text(" ")`,
    Ctrl+letter is `Key::Command('x')`.
15. Handlers are queued and run between frames with `&mut S`; they cannot return values to the view synchronously.
16. Headless needs `rune::is_headless()` guards around anything with side effects (Electron, network, tray). Scenes run before `setup`.
17. Runtime state outlives the window with `keep_running()`: `Latest` requests and your threads keep running; cancel from `on_close`.
18. Pre-release: don't expect docs and code to agree perfectly; the README is long (2,800 lines) but ahead of some APIs. When
    unsure, grep `crates/rune/src` (e.g. `el/layout.rs`, `el/visuals.rs`, `app.rs`, `tooling.rs`).
