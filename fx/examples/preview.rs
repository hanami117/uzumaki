//! Renders every effect from the built-in picture and from album covers in
//! ~/.cache/uzumaki/art/ into /tmp/uzumaki-fx/ (composited over #0e0e11 at 35%, plus raw), and
//! prints render timings. `cargo run -p uzumaki-fx --example preview`
use std::time::Instant;
use image::RgbaImage;
use uzumaki_fx::{Effect, Target, builtin, render};

fn composite(img: &RgbaImage, opacity: f32) -> RgbaImage {
    let bg = [14.0, 14.0, 17.0];
    let mut out = img.clone();
    for p in out.pixels_mut() {
        for c in 0..3 {
            p.0[c] = (bg[c] + (p.0[c] as f32 - bg[c]) * opacity).round() as u8;
        }
    }
    out
}

fn main() {
    let out = std::path::Path::new("/tmp/uzumaki-fx");
    std::fs::create_dir_all(out).unwrap();
    let dir = std::path::PathBuf::from(std::env::var("HOME").unwrap()).join(".cache/uzumaki/art");
    // Largest covers (the biggest files) make the best test pictures.
    let mut covers: Vec<_> = std::fs::read_dir(dir).unwrap().flatten().filter_map(|e| {
        let bytes = std::fs::read(e.path()).ok()?;
        let img = image::load_from_memory(&bytes).ok()?;
        (img.width() >= 400).then(|| (img.width(), e.file_name(), img.to_rgba8()))
    }).collect();
    covers.sort_by_key(|c| std::cmp::Reverse(c.0));
    covers.dedup_by_key(|c| c.2.as_raw().iter().step_by(997).copied().collect::<Vec<u8>>());
    let mut sources: Vec<(String, RgbaImage)> = vec![("builtin".into(), builtin(1360, 860))];
    for (i, (_, _, img)) in covers.into_iter().take(2).enumerate() {
        sources.push((format!("cover{i}"), img));
    }
    for (name, src) in &sources {
        for e in Effect::ALL {
            let img = render(src, e, Target { width: 1360, height: 860, unit: 1 });
            let n = format!("{name}-{}", e.label().to_lowercase());
            img.save(out.join(format!("{n}-raw.png"))).unwrap();
            composite(&img, 0.35).save(out.join(format!("{n}-35.png"))).unwrap();
        }
    }
    // Timings (best of 5).
    let src = &sources.last().unwrap().1;
    for (w, h, unit) in [(1360, 860, 1), (2720, 1720, 2)] {
        let big = if sources.len() > 1 { src.clone() } else { builtin(w, h) };
        for e in Effect::ALL {
            let best = (0..5).map(|_| {
                let t = Instant::now();
                std::hint::black_box(render(&big, e, Target { width: w, height: h, unit }));
                t.elapsed()
            }).min().unwrap();
            println!("{w}x{h} u{unit} {:<10} {:>7.1} ms", e.label(), best.as_secs_f64() * 1000.0);
        }
    }
}
