//! Офлайн-перевод RU ↔ EN моделями OPUS-MT (Helsinki-NLP, архитектура Marian) через
//! CTranslate2. Модели лежат рядом с моделями whisper: `~/.local/share/keyboop/models/opus-mt-ru-en`
//! и `opus-mt-en-ru` (int8, ~80 МБ каждая). Готовятся скриптом `tools/get_translate_models.sh`.
//!
//! Перевод живёт в своём потоке: модель грузится при первом переводе (доли секунды) и
//! остаётся в памяти. Направление выбирается по тексту: больше кириллицы — на английский.

use crate::bus::{Bus, Msg};
use ct2rs::tokenizers::sentencepiece::Tokenizer;
use ct2rs::{ComputeType, Config, TranslationOptions, Translator};
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Instant;

/// Больше не переводим: окно не для книг, а поток не должен висеть минутами.
pub const MAX_CHARS: usize = 5000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    RuEn,
    EnRu,
}

impl Dir {
    /// По тексту: кириллица весит вдвое больше латиницы — в русском тексте часто встречаются
    /// английские команды и названия («запусти make install»), а наоборот — почти никогда.
    pub fn detect(text: &str) -> Dir {
        let (mut cyr, mut lat) = (0usize, 0usize);
        for c in text.chars() {
            if matches!(c, 'а'..='я' | 'А'..='Я' | 'ё' | 'Ё') {
                cyr += 1;
            } else if c.is_ascii_alphabetic() {
                lat += 1;
            }
        }
        if cyr > 0 && cyr * 2 >= lat { Dir::RuEn } else { Dir::EnRu }
    }

    pub fn label(self) -> &'static str {
        match self {
            Dir::RuEn => "RU → EN",
            Dir::EnRu => "EN → RU",
        }
    }

    fn model_name(self) -> &'static str {
        match self {
            Dir::RuEn => "opus-mt-ru-en",
            Dir::EnRu => "opus-mt-en-ru",
        }
    }

    pub fn model_path(self) -> PathBuf {
        crate::voice::models_dir().join(self.model_name())
    }
}

/// Режет текст на предложения, сохраняя строки: `[[предл., предл.], [], [предл.]]`.
/// Marian обучен на отдельных предложениях — длинный абзац целиком он переводит хуже
/// и может «потерять» хвост.
fn split(text: &str) -> Vec<Vec<String>> {
    text.lines()
        .map(|line| {
            let mut out = Vec::new();
            let mut cur = String::new();
            let chars: Vec<char> = line.trim().chars().collect();
            for (i, &c) in chars.iter().enumerate() {
                cur.push(c);
                let end = matches!(c, '.' | '!' | '?' | '…');
                let next_space = chars.get(i + 1).is_some_and(|n| n.is_whitespace());
                let next_upper = chars[i + 1..]
                    .iter()
                    .find(|n| !n.is_whitespace())
                    .is_some_and(|n| n.is_uppercase() || matches!(n, '«' | '"' | '—' | '-'));
                if end && next_space && next_upper {
                    out.push(cur.trim().to_string());
                    cur.clear();
                }
            }
            if !cur.trim().is_empty() {
                out.push(cur.trim().to_string());
            }
            out
        })
        .collect()
}

struct Engine {
    models: Vec<(Dir, Translator<Tokenizer>)>,
    threads: usize,
}

impl Engine {
    fn translator(&mut self, dir: Dir) -> Result<&Translator<Tokenizer>, String> {
        if let Some(i) = self.models.iter().position(|(d, _)| *d == dir) {
            return Ok(&self.models[i].1);
        }
        let path = dir.model_path();
        if !path.join("model.bin").exists() {
            return Err(format!(
                "нет модели перевода {} ({}). Подготовь: tools/get_translate_models.sh",
                dir.label(),
                path.display()
            ));
        }
        let t = Instant::now();
        let cfg = Config { compute_type: ComputeType::INT8, num_threads_per_replica: self.threads, ..Default::default() };
        let tok = Tokenizer::new(&path).map_err(|e| format!("токенайзер {}: {e}", path.display()))?;
        let tr = Translator::with_tokenizer(&path, tok, &cfg).map_err(|e| format!("модель {}: {e}", path.display()))?;
        log::info!("модель перевода {} загружена за {} мс", dir.label(), t.elapsed().as_millis());
        self.models.push((dir, tr));
        Ok(&self.models.last().unwrap().1)
    }

    fn translate(&mut self, text: &str, dir: Dir) -> Result<String, String> {
        let lines = split(text);
        let flat: Vec<&String> = lines.iter().flatten().collect();
        if flat.is_empty() {
            return Ok(String::new());
        }
        let tr = self.translator(dir)?;
        let opts = TranslationOptions { beam_size: 2, ..Default::default() };
        let t = Instant::now();
        let res = tr.translate_batch(&flat, &opts, None).map_err(|e| format!("перевод: {e}"))?;
        log::info!("перевод {}: {} предл. за {} мс", dir.label(), flat.len(), t.elapsed().as_millis());
        let mut it = res.into_iter().map(|(s, _)| s);
        let out: Vec<String> = lines
            .iter()
            .map(|sents| sents.iter().map(|_| it.next().unwrap_or_default()).collect::<Vec<_>>().join(" "))
            .collect();
        Ok(out.join("\n"))
    }
}

/// Перевод одной строкой, без потока (для `keyboop translate` в терминале).
pub fn translate_once(text: &str, threads: usize) -> Result<(Dir, String), String> {
    let dir = Dir::detect(text);
    let mut e = Engine { models: Vec::new(), threads };
    e.translate(text, dir).map(|t| (dir, t))
}

/// Поток перевода. Отправленный текст переводится, результат приходит в шину.
#[derive(Clone)]
pub struct Translate {
    tx: mpsc::Sender<String>,
}

impl Translate {
    pub fn spawn(bus: Bus, threads: usize) -> Translate {
        let (tx, rx) = mpsc::channel::<String>();
        std::thread::Builder::new()
            .name("translate".into())
            .spawn(move || {
                let mut e = Engine { models: Vec::new(), threads };
                for text in rx {
                    let dir = Dir::detect(&text);
                    match e.translate(&text, dir) {
                        Ok(t) => bus.send(Msg::Translated { dir, source: text, text: t }),
                        Err(err) => bus.send(Msg::TranslateError(err)),
                    }
                }
            })
            .expect("поток перевода");
        Translate { tx }
    }

    pub fn send(&self, text: String) {
        let _ = self.tx.send(text);
    }
}

/// Показать перевод в окне (zenity), без него — уведомлением.
pub fn show(dir: Dir, source: &str, text: &str) {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let title = format!("Перевод {}", dir.label());
    let lines = text.lines().count() + text.chars().count() / 60;
    let height = (140 + lines * 22).clamp(180, 600).to_string();
    let body = format!("{text}\n\n────────── исходный текст ──────────\n{source}");
    let child = Command::new("zenity")
        .args(["--text-info", "--title", &title, "--width", "560", "--height", &height, "--ok-label", "Закрыть"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    match child {
        Ok(mut c) => {
            if let Some(mut stdin) = c.stdin.take() {
                let _ = stdin.write_all(body.as_bytes());
            }
            // Ждём в отдельном потоке, чтобы не копить зомби-процессы.
            std::thread::spawn(move || {
                let _ = c.wait();
            });
        }
        Err(_) => {
            let short: String = text.chars().take(300).collect();
            let _ = Command::new("notify-send")
                .args(["-a", "Keyboop", "-t", "15000", &title, &short])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_dir() {
        assert_eq!(Dir::detect("Привет, мир"), Dir::RuEn);
        assert_eq!(Dir::detect("Hello, world"), Dir::EnRu);
        assert_eq!(Dir::detect("Запусти make install"), Dir::RuEn);
    }

    #[test]
    fn split_sentences_keeps_lines() {
        let s = split("Первое. Второе! т.е. так\n\nThird line? Yes");
        assert_eq!(s, vec![vec!["Первое.".to_string(), "Второе! т.е. так".into()], vec![], vec!["Third line?".into(), "Yes".into()]]);
    }
}
