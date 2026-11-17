//! Keyboop для Linux/X11: исправляет текст, набранный не в той раскладке (RU ↔ EN).

mod bus;
mod config;
mod data;
mod detector;
mod engine;
mod exceptions;
mod keymap;
mod tray;
mod voice;
mod x11;

use std::io::Write;

fn usage() {
    eprintln!(
        "keyboop {ver}\n\n\
         keyboop               запустить (нужна X11-сессия)\n\
         keyboop check СЛОВО [ПРЕДЫДУЩЕЕ]   что сделает детектор с этим словом\n\
         keyboop convert ТЕКСТ              перевернуть раскладку текста\n\
         keyboop download-model [ИМЯ]       скачать модель whisper (по умолчанию из настроек)\n",
        ver = env!("CARGO_PKG_VERSION")
    );
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info,whisper_rs=warn"))
        .format(|buf, rec| writeln!(buf, "[{}] {}", rec.level(), rec.args()))
        .init();

    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        None => {
            if let Err(e) = x11::run() {
                log::error!("{e}");
                std::process::exit(1);
            }
        }
        Some("check") if args.len() >= 2 => {
            let exc = exceptions::Exceptions::load();
            let d = match detector::mixed_rescue(&args[1]) {
                detector::Decision::Keep => detector::decide(&args[1], &exc, args.get(2).map(String::as_str), None, false),
                d => d,
            };
            match d {
                detector::Decision::Keep => println!("{} → оставить", args[1]),
                detector::Decision::Convert { to_cyrillic } => {
                    let ru = &data::data().words_ru;
                    let valid = |s: &str| ru.contains(s);
                    println!("{} → {}", args[1], keymap::smart_convert(&args[1], to_cyrillic, Some(&valid)))
                }
            }
        }
        Some("convert") if args.len() >= 2 => {
            let t = args[1..].join(" ");
            use keymap::ScriptExt;
            println!("{}", keymap::convert(&t, !t.has_cyrillic()));
        }
        Some("download-model") => {
            let name = args.get(1).cloned().unwrap_or_else(|| config::Config::load().voice_model);
            match voice::download_model(&name) {
                Ok(p) => println!("готово: {}", p.display()),
                Err(e) => {
                    eprintln!("{e}");
                    std::process::exit(1);
                }
            }
        }
        Some("transcribe") if args.len() >= 2 => {
            let c = config::Config::load();
            let threads = if c.voice_threads > 0 {
                c.voice_threads
            } else {
                std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).clamp(1, 8) as i32
            };
            let vc = voice::VoiceConfig { model: c.voice_model, language: c.voice_language, threads };
            match voice::transcribe_file(vc, &args[1]) {
                Ok(t) => println!("{t}"),
                Err(e) => {
                    eprintln!("{e}");
                    std::process::exit(1);
                }
            }
        }
        _ => usage(),
    }
}
