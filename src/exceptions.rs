//! Пользовательские исключения (аналог ExceptionStore.swift).
//! Хранятся в `~/.config/keyboop/exceptions.toml`:
//! * `ignored`    — слова, которые никогда не переключать (правится руками);
//! * `force_swap` — слова, которые переключать всегда (правится руками);
//! * `learned`    — выученные на отмене авто-конверсии (пишет сама программа).

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::PathBuf;

#[derive(Default, Serialize, Deserialize)]
pub struct Exceptions {
    #[serde(default)]
    pub ignored: HashSet<String>,
    #[serde(default)]
    pub force_swap: HashSet<String>,
    #[serde(default)]
    pub learned: HashSet<String>,
}

impl Exceptions {
    pub fn path() -> PathBuf {
        crate::config::config_dir().join("exceptions.toml")
    }

    pub fn load() -> Self {
        let p = Self::path();
        match std::fs::read_to_string(&p) {
            Ok(s) => match toml::from_str::<Exceptions>(&s) {
                Ok(mut e) => {
                    let lower = |s: HashSet<String>| s.into_iter().map(|w| w.to_lowercase()).collect();
                    e.ignored = lower(e.ignored);
                    e.force_swap = lower(e.force_swap);
                    e.learned = lower(e.learned);
                    e
                }
                Err(err) => {
                    log::warn!("не разобрал {}: {err} — исключения пустые", p.display());
                    Self::default()
                }
            },
            Err(_) => Self::default(),
        }
    }

    pub fn save(&self) {
        if cfg!(test) {
            return; // тесты не пишут в настоящий конфиг
        }
        let p = Self::path();
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let sorted = |s: &HashSet<String>| {
            let mut v: Vec<_> = s.iter().cloned().collect();
            v.sort();
            v
        };
        #[derive(Serialize)]
        struct Out {
            ignored: Vec<String>,
            force_swap: Vec<String>,
            learned: Vec<String>,
        }
        let out = Out { ignored: sorted(&self.ignored), force_swap: sorted(&self.force_swap), learned: sorted(&self.learned) };
        let body = format!(
            "# Исключения Keyboop.\n# ignored — никогда не переключать, force_swap — переключать всегда.\n# learned — выучено на отмене (хоткей сразу после авто-исправления).\n\n{}",
            toml::to_string_pretty(&out).unwrap_or_default()
        );
        if let Err(e) = std::fs::write(&p, body) {
            log::warn!("не сохранил исключения: {e}");
        }
    }
}
