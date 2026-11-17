//! Настройки из `~/.config/keyboop/config.toml`. Файл создаётся с умолчаниями при первом запуске.

use serde::Deserialize;
use std::path::PathBuf;

pub const DEFAULT_CONFIG: &str = r#"# Настройки Keyboop для Linux (X11).
# После правки: systemctl --user restart keyboop

# Автоматически исправлять слово, набранное не в той раскладке, по пробелу.
auto = true

# Горячие клавиши: модификаторы (shift, ctrl, alt, super) + имя клавиши X11
# (как в xev: Pause, Scroll_Lock, F12, grave, space...). Пустая строка — выключено.

# Исправить текущее слово (или последнее, если уже нажат пробел).
# Сразу после авто-исправления — отменяет его и запоминает слово как исключение.
hotkey_word = "Pause"

# Исправить всю набранную фразу (с последнего клика/Enter).
hotkey_phrase = "shift+Pause"

# Включить/выключить авто-исправление.
hotkey_toggle = "ctrl+Pause"

# Перевести выделенный текст (RU ↔ EN, офлайн) и показать в окне.
# Модели: tools/get_translate_models.sh
hotkey_translate = "alt+Pause"

# Программы, где авто-исправление выключено (класс окна, см. `xprop WM_CLASS`).
# Горячие клавиши в них работают.
exclude_apps = []

# --- Голосовой набор ---
voice = true

# Держи клавишу — говори — отпусти: текст напечатается. Имя клавиши X11.
# Control_R — правый Ctrl. Если нажать с ней другую клавишу (Ctrl+C), запись отменяется.
voice_hotkey = "Control_R"

# Модель whisper. Замер на i5-8350U, фраза в несколько секунд:
#   small — ~5.5 с, хорошо по-русски (по умолчанию);
#   base  — ~1.5 с, по-русски заметно хуже.
# Скачать другую: keyboop download-model base   (или пункт в меню значка)
voice_model = "small"

# Язык: "ru", "en" или "auto". "auto" определяет язык по речи, но на процессоре
# распознавание идёт вдвое дольше (лишний проход модели). Английские слова внутри
# русской речи whisper обычно и так пишет латиницей.
voice_language = "ru"

# Ставить точку в конце и заглавную букву в начале.
voice_final_period = true
voice_capitalize = true

# Звук начала и конца записи.
voice_sounds = true

# Потоков для распознавания (0 — по числу ядер).
voice_threads = 0

# Писать в журнал сами слова (для отладки). По умолчанию в журнал текст не попадает.
log_text = false
"#;

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct Config {
    pub auto: bool,
    pub hotkey_word: String,
    pub hotkey_phrase: String,
    pub hotkey_toggle: String,
    pub hotkey_translate: String,
    pub exclude_apps: Vec<String>,
    pub log_text: bool,
    pub voice: bool,
    pub voice_hotkey: String,
    pub voice_model: String,
    pub voice_language: String,
    pub voice_final_period: bool,
    pub voice_capitalize: bool,
    pub voice_sounds: bool,
    pub voice_threads: i32,
}

// Значения должны совпадать с DEFAULT_CONFIG (это проверяет тест ниже). Разбирать сам
// DEFAULT_CONFIG здесь нельзя: `#[serde(default)]` зовёт `default()` → бесконечная рекурсия.
impl Default for Config {
    fn default() -> Self {
        Config {
            auto: true,
            hotkey_word: "Pause".into(),
            hotkey_phrase: "shift+Pause".into(),
            hotkey_toggle: "ctrl+Pause".into(),
            hotkey_translate: "alt+Pause".into(),
            exclude_apps: vec![],
            log_text: false,
            voice: true,
            voice_hotkey: "Control_R".into(),
            voice_model: "small".into(),
            voice_language: "ru".into(),
            voice_final_period: true,
            voice_capitalize: true,
            voice_sounds: true,
            voice_threads: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn default_matches_file() {
        let parsed: super::Config = toml::from_str(super::DEFAULT_CONFIG).unwrap();
        assert_eq!(format!("{parsed:?}"), format!("{:?}", super::Config::default()));
    }
}

pub fn config_dir() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config"))
        .join("keyboop")
}

impl Config {
    pub fn load() -> Self {
        let path = config_dir().join("config.toml");
        match std::fs::read_to_string(&path) {
            Ok(s) => toml::from_str(&s).unwrap_or_else(|e| {
                log::error!("ошибка в {}: {e} — беру умолчания", path.display());
                Config::default()
            }),
            Err(_) => {
                let _ = std::fs::create_dir_all(config_dir());
                if std::fs::write(&path, DEFAULT_CONFIG).is_ok() {
                    log::info!("создал {}", path.display());
                }
                Config::default()
            }
        }
    }
}
