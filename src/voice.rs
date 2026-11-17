//! Голосовой набор: запись с микрофона (cpal → ALSA → PipeWire) и распознавание whisper.cpp
//! на процессоре. Всё локально, в сеть уходит только явное скачивание модели.
//!
//! Модель грузится один раз при старте и живёт в памяти (small ≈ 0.5 ГБ), чтобы первая
//! диктовка не ждала загрузки. Параметры распознавания, затравка для пунктуации и фильтр
//! фраз-призраков перенесены из WhisperBridge/WhisperPrompt/WhisperGhosts.swift.

use crate::bus::{Bus, Msg, VoiceStatus};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

const RATE: u32 = 16_000;
/// Ревизия репозитория моделей, та же, что у macOS-версии (ModelDownloader.swift).
const MODEL_REVISION: &str = "5359861c739e955e79d9a303bcbc70fb988958b1";

pub fn models_dir() -> PathBuf {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/share"))
        .join("keyboop/models")
}

pub fn model_path(name: &str) -> PathBuf {
    models_dir().join(format!("ggml-{name}.bin"))
}

pub fn model_url(name: &str) -> String {
    format!("https://huggingface.co/ggerganov/whisper.cpp/resolve/{MODEL_REVISION}/ggml-{name}.bin")
}

/// Скачивает модель curl'ом (с докачкой и прогрессом в журнале). Блокирует поток.
pub fn download_model(name: &str) -> Result<PathBuf, String> {
    let dst = model_path(name);
    std::fs::create_dir_all(models_dir()).map_err(|e| e.to_string())?;
    let part = dst.with_extension("bin.part");
    log::info!("скачиваю {} → {}", model_url(name), dst.display());
    let st = std::process::Command::new("curl")
        .args(["-L", "--fail", "-C", "-", "--progress-bar", "-o"])
        .arg(&part)
        .arg(model_url(name))
        .status()
        .map_err(|e| format!("не запустил curl: {e}"))?;
    if !st.success() {
        return Err(format!("curl завершился с ошибкой ({st}); неверное имя модели?"));
    }
    std::fs::rename(&part, &dst).map_err(|e| e.to_string())?;
    Ok(dst)
}

#[derive(Clone, Debug)]
pub struct VoiceConfig {
    pub model: String,
    pub language: String,
    pub threads: i32,
}

pub enum Cmd {
    Start,
    Stop,
    Cancel,
    Reload,
}

#[derive(Clone)]
pub struct Voice {
    tx: mpsc::Sender<Cmd>,
}

impl Voice {
    pub fn spawn(cfg: VoiceConfig, bus: Bus) -> Voice {
        let (tx, rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("voice".into())
            .spawn(move || Worker { cfg, bus, ctx: None, rec: None }.run(rx))
            .expect("поток голоса");
        Voice { tx }
    }

    pub fn send(&self, c: Cmd) {
        let _ = self.tx.send(c);
    }
}

struct Recording {
    _stream: cpal::Stream,
    samples: Arc<Mutex<Vec<f32>>>,
    rate: u32,
    started: Instant,
}

struct Worker {
    cfg: VoiceConfig,
    bus: Bus,
    ctx: Option<WhisperContext>,
    rec: Option<Recording>,
}

impl Worker {
    fn status(&self, s: VoiceStatus) {
        self.bus.send(Msg::VoiceStatus(s));
    }

    fn load(&mut self) {
        self.ctx = None;
        let path = model_path(&self.cfg.model);
        if !path.exists() {
            log::warn!("нет модели {} — скачай: keyboop download-model {}", path.display(), self.cfg.model);
            self.status(VoiceStatus::NoModel);
            return;
        }
        self.status(VoiceStatus::Loading);
        let t = Instant::now();
        match WhisperContext::new_with_params(&path, WhisperContextParameters::default()) {
            Ok(ctx) => {
                log::info!("модель «{}» загружена за {} мс", self.cfg.model, t.elapsed().as_millis());
                self.ctx = Some(ctx);
                self.status(VoiceStatus::Idle);
            }
            Err(e) => {
                self.bus.send(Msg::VoiceError(format!("не загрузил модель: {e}")));
                self.status(VoiceStatus::NoModel);
            }
        }
    }

    fn run(mut self, rx: mpsc::Receiver<Cmd>) {
        whisper_rs::install_logging_hooks();
        self.load();
        for cmd in rx {
            match cmd {
                Cmd::Reload => self.load(),
                Cmd::Start => {
                    if self.ctx.is_none() || self.rec.is_some() {
                        continue;
                    }
                    match start_recording() {
                        Ok(r) => {
                            self.rec = Some(r);
                            self.status(VoiceStatus::Recording);
                        }
                        Err(e) => self.bus.send(Msg::VoiceError(format!("микрофон: {e}"))),
                    }
                }
                Cmd::Cancel => {
                    if self.rec.take().is_some() {
                        log::info!("диктовка отменена");
                        self.status(VoiceStatus::Idle);
                    }
                }
                Cmd::Stop => {
                    let Some(rec) = self.rec.take() else { continue };
                    let secs = rec.started.elapsed().as_secs_f32();
                    let raw = std::mem::take(&mut *rec.samples.lock().unwrap());
                    let rate = rec.rate;
                    drop(rec); // закрываем микрофон до распознавания
                    if secs < 0.4 {
                        self.status(VoiceStatus::Idle);
                        continue;
                    }
                    self.status(VoiceStatus::Transcribing);
                    let samples = resample(&raw, rate);
                    let t = Instant::now();
                    match self.transcribe(&samples) {
                        Ok(text) => {
                            log::info!(
                                "распознано за {} мс ({:.1} с звука): {} симв.",
                                t.elapsed().as_millis(),
                                samples.len() as f32 / RATE as f32,
                                text.chars().count()
                            );
                            if !text.is_empty() {
                                self.bus.send(Msg::VoiceText(text));
                            }
                        }
                        Err(e) => self.bus.send(Msg::VoiceError(format!("распознавание: {e}"))),
                    }
                    self.status(VoiceStatus::Idle);
                }
            }
        }
    }

    fn transcribe(&self, samples: &[f32]) -> Result<String, Box<dyn std::error::Error>> {
        let ctx = self.ctx.as_ref().ok_or("модель не загружена")?;
        let mut state = ctx.create_state()?;
        let threads = self.cfg.threads;
        // В «авто» язык определяем ДО расшифровки, и затравка идёт на нём одном (WhisperBridge.swift).
        let lang: String = if self.cfg.language == "auto" {
            state.pcm_to_mel(samples, threads as usize)?;
            let (id, probs) = state.lang_detect(0, threads as usize)?;
            let p = probs.get(id as usize).copied().unwrap_or(0.0);
            match whisper_rs::get_lang_str(id) {
                Some(l) if p >= CONFIDENT_LANGUAGE => l.to_string(),
                _ => "auto".into(),
            }
        } else {
            self.cfg.language.clone()
        };
        let prompt = prompt_for(&lang);
        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);
        params.set_print_special(false);
        params.set_no_timestamps(true);
        params.set_translate(false);
        params.set_suppress_blank(true);
        params.set_no_context(true);
        params.set_single_segment(false);
        params.set_temperature(0.0);
        params.set_temperature_inc(0.2);
        params.set_entropy_thold(2.4);
        params.set_no_speech_thold(0.6);
        params.set_n_threads(threads);
        params.set_language(Some(&lang));
        if let Some(p) = &prompt {
            params.set_initial_prompt(p);
        }
        state.full(params, samples)?;
        let mut text = String::new();
        for seg in state.as_iter() {
            text.push_str(&seg.to_str_lossy()?);
        }
        let text = strip_echo(&text, prompt.as_deref().unwrap_or(""));
        Ok(clean_ghosts(&text))
    }
}

/// Распознать файл (любой формат, который понимает ffmpeg) — для проверки без микрофона.
pub fn transcribe_file(cfg: VoiceConfig, path: &str) -> Result<String, Box<dyn std::error::Error>> {
    let out = std::process::Command::new("ffmpeg")
        .args(["-v", "error", "-i", path, "-f", "f32le", "-ac", "1", "-ar", "16000", "-"])
        .output()?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).into());
    }
    let samples: Vec<f32> = out.stdout.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect();
    let (bus, _rx) = crate::bus::channel()?;
    let mut w = Worker { cfg, bus, ctx: None, rec: None };
    whisper_rs::install_logging_hooks();
    w.load();
    let t = Instant::now();
    let text = w.transcribe(&samples)?;
    log::info!("{:.1} с звука распознано за {} мс", samples.len() as f32 / RATE as f32, t.elapsed().as_millis());
    Ok(text)
}

fn start_recording() -> Result<Recording, String> {
    let host = cpal::default_host();
    let dev = host.default_input_device().ok_or("нет устройства записи")?;
    // Просим сразу 16 кГц (PipeWire сам пересэмплирует), иначе — что устройство даёт по умолчанию.
    let cfg = dev
        .supported_input_configs()
        .map_err(|e| e.to_string())?
        .filter(|c| matches!(c.sample_format(), cpal::SampleFormat::F32 | cpal::SampleFormat::I16))
        .find_map(|c| c.try_with_sample_rate(RATE))
        .map(Ok)
        .unwrap_or_else(|| dev.default_input_config().map_err(|e| e.to_string()))?;
    let rate = cfg.sample_rate();
    let channels = cfg.channels() as usize;
    let samples = Arc::new(Mutex::new(Vec::with_capacity(rate as usize * 30)));
    let err = |e| log::warn!("ошибка потока записи: {e}");
    let stream = match cfg.sample_format() {
        cpal::SampleFormat::F32 => {
            let s = samples.clone();
            dev.build_input_stream(
                cfg.into(),
                move |data: &[f32], _: &_| push_mono(&s, data.iter().copied(), channels),
                err,
                None,
            )
        }
        cpal::SampleFormat::I16 => {
            let s = samples.clone();
            dev.build_input_stream(
                cfg.into(),
                move |data: &[i16], _: &_| push_mono(&s, data.iter().map(|v| *v as f32 / 32768.0), channels),
                err,
                None,
            )
        }
        f => return Err(format!("формат {f:?} не поддержан")),
    }
    .map_err(|e| e.to_string())?;
    stream.play().map_err(|e| e.to_string())?;
    log::info!("запись: {rate} Гц, каналов {channels}");
    Ok(Recording { _stream: stream, samples, rate, started: Instant::now() })
}

fn push_mono(dst: &Mutex<Vec<f32>>, data: impl Iterator<Item = f32>, channels: usize) {
    let v: Vec<f32> = data.collect();
    let mut d = dst.lock().unwrap();
    for frame in v.chunks(channels.max(1)) {
        d.push(frame.iter().sum::<f32>() / frame.len() as f32);
    }
}

/// В 16 кГц усреднением по окну (простая защита от наложения частот при понижении).
fn resample(input: &[f32], rate: u32) -> Vec<f32> {
    if rate == RATE {
        return input.to_vec();
    }
    let ratio = rate as f64 / RATE as f64;
    let n = (input.len() as f64 / ratio) as usize;
    (0..n)
        .map(|j| {
            let a = (j as f64 * ratio) as usize;
            let b = (((j + 1) as f64 * ratio) as usize).clamp(a + 1, input.len());
            input[a..b].iter().sum::<f32>() / (b - a) as f32
        })
        .collect()
}

// ---------- затравка и чистка текста (WhisperPrompt.swift, WhisperGhosts.swift) ----------

const PROMPT_RU: &str = "Привет! Как дела? Сегодня хорошая погода, но, кажется, скоро пойдёт дождь. Ну что ж, подождём — время ещё есть.";
const PROMPT_EN: &str = "Hi! How are you? The weather is nice today, but it looks like it might rain. Well, let's wait — there's still time.";
const CONFIDENT_LANGUAGE: f32 = 0.5;

fn prompt_for(lang: &str) -> Option<String> {
    match lang {
        "ru" => Some(PROMPT_RU.into()),
        "en" => Some(PROMPT_EN.into()),
        "auto" => Some(format!("{PROMPT_RU} {PROMPT_EN}")),
        _ => None,
    }
}

/// Whisper иногда возвращает саму затравку в начале текста — срезаем.
fn strip_echo(text: &str, prompt: &str) -> String {
    let mut r = text.trim().to_string();
    if prompt.is_empty() {
        return r;
    }
    let mut pieces: Vec<String> = Vec::new();
    let mut rest = prompt.to_string();
    for block in [PROMPT_RU, PROMPT_EN] {
        if rest.contains(block) {
            pieces.push(block.into());
            rest = rest.replace(block, " ");
        }
    }
    let rem = rest.trim();
    if !rem.is_empty() {
        pieces.push(rem.into());
    }
    pieces.sort_by_key(|p| std::cmp::Reverse(p.chars().count()));
    while let Some(p) = pieces.iter().find(|p| r.starts_with(p.as_str())) {
        r = r[p.len()..].trim().to_string();
    }
    r
}

fn ghost_regexes() -> &'static Vec<regex::Regex> {
    static R: std::sync::OnceLock<Vec<regex::Regex>> = std::sync::OnceLock::new();
    R.get_or_init(|| {
        [
            r"^субтитры (создал|создавал|сделал|делал|подготовил|подготовлены|предоставлены|by)\b.*$",
            r"^редактор субтитров\b.*$",
            r"^продолжение следует$",
            r"^спасибо за просмотр$",
            r"^спасибо за внимание$",
            r"^подписывайтесь на канал$",
            r"^подписывайся на канал$",
            r"^ставьте лайк$",
            r"^не забудьте подписаться$",
            r"^thanks? (you )?for watching$",
            r"^subtitles by\b.*$",
            r"^subtitles? and translation by\b.*$",
            r"^please subscribe$",
            r"^amara\.org\b.*$",
            r"^www\.[a-z0-9.-]+$",
        ]
        .iter()
        .map(|p| regex::RegexBuilder::new(p).case_insensitive(true).build().unwrap())
        .collect()
    })
}

fn is_ghost(sentence: &str) -> bool {
    let s = sentence.trim_matches(|c: char| " \t\n.,!?…-–—:;\"'«»()".contains(c));
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    !s.is_empty() && ghost_regexes().iter().any(|r| r.is_match(&s))
}

/// Фразы-призраки («Продолжение следует…», «Субтитры сделал…») в начале и в конце — убрать.
pub fn clean_ghosts(text: &str) -> String {
    let split_re = regex::Regex::new(r"([.!?…]+)\s+").unwrap();
    let sentences: Vec<String> = text
        .lines()
        .flat_map(|l| split_re.replace_all(l, "$1\u{1}").split('\u{1}').map(str::to_string).collect::<Vec<_>>())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if sentences.is_empty() {
        return text.trim().to_string();
    }
    let mut kept = sentences.clone();
    while kept.first().is_some_and(|s| is_ghost(s)) {
        kept.remove(0);
    }
    while kept.last().is_some_and(|s| is_ghost(s)) {
        kept.pop();
    }
    kept.retain(|s| s.chars().any(|c| c.is_alphanumeric()));
    if kept.len() == sentences.len() {
        return text.trim().to_string();
    }
    log::info!("whisper: убрано фраз-призраков: {}", sentences.len() - kept.len());
    kept.join(" ").trim().to_string()
}

/// Подготовка к печати: символы, которых нет ни в одной раскладке, заменяем на набираемые.
pub fn normalize_for_typing(text: &str, final_period: bool, capitalize: bool) -> String {
    let mut s: String = text
        .chars()
        .map(|c| match c {
            '—' | '–' | '−' => '-',
            '«' | '»' | '“' | '”' | '„' => '"',
            '’' | '‘' => '\'',
            '\u{00A0}' | '\u{2009}' | '\u{202F}' => ' ',
            c => c,
        })
        .collect::<String>()
        .replace('…', "...");
    s = s.trim().to_string();
    if !final_period && s.ends_with('.') && !s.ends_with("..") {
        s.pop();
    }
    if !capitalize {
        let mut cs = s.chars();
        if let Some(f) = cs.next() {
            s = f.to_lowercase().chain(cs).collect();
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ghosts() {
        assert_eq!(clean_ghosts("Привет, как дела? Продолжение следует..."), "Привет, как дела?");
        assert_eq!(clean_ghosts("Субтитры сделал DimaTorzok"), "");
        assert_eq!(clean_ghosts("Всё хорошо."), "Всё хорошо.");
    }

    #[test]
    fn echo() {
        let p = prompt_for("ru").unwrap();
        assert_eq!(strip_echo(&format!("{PROMPT_RU} Тест"), &p), "Тест");
    }

    #[test]
    fn normalize() {
        assert_eq!(normalize_for_typing("Да — «ок»…", true, true), "Да - \"ок\"...");
        assert_eq!(normalize_for_typing("Привет.", false, false), "привет");
    }
}
