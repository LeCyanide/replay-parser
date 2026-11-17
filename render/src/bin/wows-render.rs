//! wows-render <replay.wowsreplay | -> --out <file.mp4 | -> --data <dir>
//!             [--speed 32 | --duration <s>] [--max-mb 8] [--fps 30]
//!             [--width 1080] [--height 720] [--progress json]
//!             [--frames <dir> [--every N]] [--no-hold] [--no-grid]
//!
//! `-` reads the replay from stdin / writes the MP4 to stdout. With
//! `--progress json`, stderr carries one JSON object per line:
//! `{"stage","done","total"}`, then `{"stage":"done","bytes","frames","warnings"}`;
//! a failure ends with `{"error":{"code","message"}}` and a non-zero exit.
//! `--frames <dir>` writes every Nth frame as PNG instead of encoding (for
//! checking the picture).

use std::io::Read;
use std::io::Write;
use std::path::PathBuf;

use replay_render::DataDir;
use replay_render::Progress;
use replay_render::RenderError;
use replay_render::RenderOptions;

struct Args {
    input: String,
    out: Option<String>,
    data: PathBuf,
    opts: RenderOptions,
    json: bool,
    frames: Option<PathBuf>,
    every: u32,
}

fn parse() -> Result<Args, String> {
    let mut it = std::env::args().skip(1);
    let (mut input, mut out, mut data, mut frames) = (None, None, None, None);
    let mut opts = RenderOptions::default();
    let (mut json, mut every) = (false, 30);
    while let Some(a) = it.next() {
        let mut val = |name: &str| it.next().ok_or_else(|| format!("{name} needs a value"));
        let num = |v: String, name: &str| v.parse::<f32>().map_err(|_| format!("{name}: not a number: {v}"));
        match a.as_str() {
            "--out" | "-o" => out = Some(val("--out")?),
            "--data" => data = Some(PathBuf::from(val("--data")?)),
            "--duration" => opts.duration = Some(num(val("--duration")?, "--duration")?),
            "--speed" => opts.speed = num(val("--speed")?, "--speed")?,
            "--max-mb" => opts.max_bytes = (num(val("--max-mb")?, "--max-mb")? as f64 * 1e6) as u64,
            "--fps" => opts.fps = num(val("--fps")?, "--fps")? as u32,
            "--width" => opts.width = num(val("--width")?, "--width")? as u32,
            "--height" => opts.height = num(val("--height")?, "--height")? as u32,
            "--progress" => json = val("--progress")? == "json",
            "--frames" => frames = Some(PathBuf::from(val("--frames")?)),
            "--every" => every = (num(val("--every")?, "--every")? as u32).max(1),
            "--no-hold" => opts.hold_end = false,
            "--no-grid" => opts.grid = false,
            "-h" | "--help" => return Err(String::new()),
            s if input.is_none() && (s == "-" || !s.starts_with('-')) => input = Some(s.to_owned()),
            s => return Err(format!("unexpected argument {s}")),
        }
    }
    let input = input.ok_or("missing replay path (or - for stdin)")?;
    let data = data.ok_or("missing --data <dir>")?;
    if out.is_none() && frames.is_none() {
        return Err("missing --out <file.mp4 | -> (or --frames <dir>)".into());
    }
    Ok(Args { input, out, data, opts, json, frames, every })
}

fn main() {
    let args = match parse() {
        Ok(a) => a,
        Err(e) => {
            if !e.is_empty() {
                eprintln!("error: {e}");
            }
            eprintln!("{}", USAGE);
            std::process::exit(2);
        }
    };
    let json = args.json;
    if let Err(e) = run(args) {
        if json {
            let line = serde_json::json!({ "error": { "code": e.code(), "message": e.to_string() } });
            eprintln!("{line}");
        } else {
            eprintln!("error: {e}");
        }
        std::process::exit(1);
    }
}

const USAGE: &str = "usage: wows-render <replay.wowsreplay | -> --out <file.mp4 | -> --data <dir>
       [--speed 32 | --duration <s>] [--max-mb 8] [--fps 30] [--width 1080] [--height 720] [--progress json]
       [--frames <png dir> [--every 30]] [--no-hold] [--no-grid]";

fn run(a: Args) -> Result<(), RenderError> {
    let bytes = if a.input == "-" {
        let mut b = Vec::new();
        std::io::stdin().lock().read_to_end(&mut b)?;
        b
    } else {
        std::fs::read(&a.input)?
    };
    let data = DataDir::new(&a.data);
    let json = a.json;
    let mut last = (None, 0u32);
    let mut progress = |p: Progress| {
        if !json {
            return;
        }
        // At most ~50 lines per stage.
        let step = (p.total / 50).max(1);
        if last.0 != Some(p.stage) || p.done == p.total || p.done >= last.1 + step {
            last = (Some(p.stage), p.done);
            eprintln!("{}", serde_json::to_string(&p).unwrap_or_default());
        }
    };
    let (bytes_out, summary) = if let Some(dir) = &a.frames {
        a.opts.validate()?;
        std::fs::create_dir_all(dir)?;
        let mut n = 0u32;
        let summary = replay_render::scene::render(
            &bytes,
            &data,
            &a.opts,
            &mut progress,
            &mut |pm: &replay_render::tiny_skia::Pixmap| {
                if n.is_multiple_of(a.every) {
                    pm.save_png(dir.join(format!("frame-{n:05}.png")))
                        .map_err(|e| RenderError::Internal(e.to_string()))?;
                }
                n += 1;
                Ok(())
            },
        )?;
        (0, summary)
    } else {
        let (mp4, summary) = replay_render::render_mp4(&bytes, &data, &a.opts, &mut progress)?;
        match a.out.as_deref() {
            Some("-") => {
                let mut o = std::io::stdout().lock();
                o.write_all(&mp4)?;
                o.flush()?;
            }
            Some(path) => std::fs::write(path, &mp4)?,
            None => {}
        }
        (mp4.len(), summary)
    };
    if json {
        let line = serde_json::json!({
            "stage": "done", "bytes": bytes_out, "frames": summary.frames, "warnings": summary.warnings,
        });
        eprintln!("{line}");
    } else {
        for w in &summary.warnings {
            eprintln!("warning: {w}");
        }
        eprintln!(
            "{} frames, {:.0} s of battle at ×{:.0}{}",
            summary.frames,
            summary.battle_seconds,
            summary.speed,
            if bytes_out > 0 { format!(", {bytes_out} bytes") } else { String::new() }
        );
    }
    Ok(())
}
