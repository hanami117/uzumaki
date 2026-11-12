//! One line of text that scrolls when it doesn't fit, like a long title in Music's player.
//! It rests at the start, then slides left until a second copy has taken the first one's
//! place, then rests again. Text that fits is drawn as it is.
//!
//! The slide is a `rune::Tween` on the line's offset, so it runs at render time with no wakes
//! and no view runs. `step` (from `poll`) only starts and ends slides: two view runs a cycle.

use std::time::Duration;

use rune::{El, TextStyle, Tween, el, row, text};

use crate::state::State;

/// Rest at the start before each slide, seconds.
const REST: f64 = 3.0;
/// Slide speed, points a second.
const SPEED: f32 = 30.0;
/// Space between the end of the text and its copy.
const GAP: f32 = 48.0;
/// Soft edges: the right one whenever the text is cut, the left one while it slides out.
const FADE: f32 = 16.0;
/// A slide is ended this long after its tween should have, which starts on the next frame.
const SETTLE: f64 = 0.1;

/// Every scrolling line, by key. Lives in `UiState`.
#[derive(Default)]
pub struct Marquees {
    lines: Vec<Line>,
}

struct Line {
    key: &'static str,
    /// Width of the line's box at its last layout.
    box_w: f32,
    /// The text it shows, and that text's natural width.
    text: String,
    text_w: f32,
    /// Poll time when the current rest or slide began.
    since: f64,
    slide: Option<Tween<[f32; 2]>>,
}

impl Line {
    fn overflows(&self) -> bool {
        overflows(self.text_w, self.box_w)
    }

    fn slide_secs(&self) -> f64 {
        ((self.text_w + GAP) / SPEED) as f64
    }

    /// A new text: start over from the rest. True if a slide was running.
    fn retext(&mut self, text: &str, font: TextStyle, t: f64) -> bool {
        self.text = text.to_string();
        self.text_w = measure(text, font).0;
        self.since = t;
        self.slide.take().is_some()
    }
}

fn measure(text: &str, font: TextStyle) -> (f32, f32) {
    rune::text::measure(text, &[], font, None)
}

fn overflows(text_w: f32, box_w: f32) -> bool {
    box_w > 0.0 && text_w > box_w + 0.5
}

impl Marquees {
    fn get(&self, key: &str) -> Option<&Line> {
        self.lines.iter().find(|l| l.key == key)
    }

    fn get_mut(&mut self, key: &'static str, t: f64) -> &mut Line {
        let i = match self.lines.iter().position(|l| l.key == key) {
            Some(i) => i,
            None => {
                self.lines.push(Line { key, box_w: 0.0, text: String::new(), text_w: 0.0, since: t, slide: None });
                self.lines.len() - 1
            }
        };
        &mut self.lines[i]
    }

    /// A layout of line `key`'s box. True when whether it overflows changed, so the view must
    /// run again (one copy or two).
    fn layout(&mut self, key: &'static str, text: &str, font: TextStyle, box_w: f32) -> bool {
        let l = self.get_mut(key, 0.0);
        let before = l.overflows();
        if l.text != text {
            l.retext(text, font, 0.0);
        }
        l.box_w = box_w;
        if !l.overflows() {
            l.slide = None;
        }
        l.overflows() != before
    }

    /// Moves line `key`, which shows `text` now, along its cycle at poll time `t`, and asks for
    /// the poll that ends the current rest or slide. True when the view must run.
    pub fn step(&mut self, key: &'static str, text: &str, font: TextStyle, t: f64, tx: &mut rune::Tx) -> bool {
        let l = self.get_mut(key, t);
        if l.text != text {
            l.retext(text, font, t);
            return true;
        }
        if !l.overflows() {
            return l.slide.take().is_some();
        }
        let elapsed = t - l.since;
        let (wait, changed) = match l.slide {
            None if elapsed >= REST => {
                let secs = l.slide_secs();
                l.slide = Some(Tween::linear([0.0, 0.0], [-(l.text_w + GAP), 0.0], Duration::from_secs_f64(secs)));
                l.since = t;
                (secs + SETTLE, true)
            }
            None => (REST - elapsed, false),
            Some(_) if elapsed >= l.slide_secs() + SETTLE => {
                // The copy now sits where the text began: back to the start, which looks the same.
                l.slide = None;
                l.since = t;
                (REST, true)
            }
            Some(_) => (l.slide_secs() + SETTLE - elapsed, false),
        };
        tx.wake_after(Duration::from_secs_f64(wait.max(0.0)));
        changed
    }

    /// Lines stop where they are and rest from the start again (a hidden player bar).
    pub fn rest_all(&mut self) -> bool {
        let mut changed = false;
        for l in &mut self.lines {
            changed |= l.slide.take().is_some();
        }
        changed
    }
}

/// Line `key` showing `txt`: plain when it fits, else two copies in a strip that slides.
/// Its height is the font's line height; give it its width from the parent.
pub fn line(s: &State, key: &'static str, txt: String, font: TextStyle, color: rune::Color, still: bool) -> El {
    let (text_w, h) = measure(&txt, font);
    let l = s.ui.marquee.get(key);
    let fits = !overflows(text_w, l.map_or(0.0, |l| l.box_w));
    let slide = l.filter(|l| l.text == txt && !still).and_then(|l| l.slide.as_ref());
    let txt2 = txt.clone();
    let b = el().key(key).w_full().min_w(0.0).h(h).on_layout_tx(move |s: &mut State, r: [f32; 4], tx: &mut rune::Tx| {
        if !s.ui.marquee.layout(key, &txt2, font, r[2]) {
            tx.live_only();
        }
    });
    if fits || still {
        return b.child(text(txt).font(font).color(color).truncate().w_full().min_w(0.0));
    }
    let copy = |t: String| text(t).font(font).color(color);
    let mut strip = row().absolute(0.0, 0.0).gap(GAP).child(copy(txt.clone())).child(copy(txt));
    if let Some(tw) = slide {
        strip = strip.live_offset(tw);
    }
    b.clip_fade(if slide.is_some() { FADE } else { 0.0 }, 0.0, FADE, 0.0).child(strip)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tx() -> rune::Tx {
        rune::Tx::default()
    }

    #[test]
    fn a_line_that_fits_never_slides() {
        let mut m = Marquees::default();
        let font = TextStyle::new(14.0, 600.0);
        m.layout("t", "Short", font, 400.0);
        let mut tx = tx();
        m.step("t", "Short", font, 0.0, &mut tx);
        assert!(!m.step("t", "Short", font, 10.0, &mut tx));
        assert!(m.get("t").unwrap().slide.is_none());
    }

    #[test]
    fn a_long_line_rests_slides_and_rests_again() {
        let mut m = Marquees::default();
        let font = TextStyle::new(14.0, 600.0);
        let long = "A very long song title that will never fit in a narrow player bar";
        let mut tx = tx();
        assert!(m.step("t", long, font, 0.0, &mut tx), "new text: the view runs");
        assert!(m.layout("t", long, font, 120.0), "now it overflows: two copies");
        assert!(!m.step("t", long, font, 1.0, &mut tx), "resting");
        assert!(m.step("t", long, font, REST, &mut tx), "the slide starts");
        let secs = m.get("t").unwrap().slide_secs();
        assert!(m.get("t").unwrap().slide.is_some());
        assert!(!m.step("t", long, font, REST + secs * 0.5, &mut tx), "mid-slide: nothing to rebuild");
        assert!(m.step("t", long, font, REST + secs + SETTLE, &mut tx), "the slide ends");
        assert!(m.get("t").unwrap().slide.is_none());
        // A new song starts over from the rest.
        m.step("t", long, font, 20.0, &mut tx);
        assert!(m.step("t", "Another very long title that also overflows the bar", font, 20.5, &mut tx));
        assert!(m.get("t").unwrap().slide.is_none());
        // Widening the bar until it fits stops it.
        assert!(m.layout("t", "Another very long title that also overflows the bar", font, 2000.0));
    }
}
