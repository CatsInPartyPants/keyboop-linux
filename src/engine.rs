//! Модель набранного текста и решения «что заменить». Ничего не знает про X11: на вход —
//! события клавиатуры, на выход — `Action` (сколько стереть, что напечатать, какую раскладку
//! включить). Поэтому вся логика проверяется обычными тестами.

use crate::detector::{self, Decision};
use crate::exceptions::Exceptions;
use crate::keymap::{self, ScriptExt};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Input {
    /// Печатный символ, уже декодированный текущей раскладкой.
    Char(char),
    /// Backspace; `word = true` для Ctrl+Backspace (стирает неизвестно сколько).
    Backspace { word: bool },
    Space,
    /// Enter/Tab: слово завершено, но авто не трогаем (сообщение уже могло уйти, фокус — сместиться).
    Break,
    /// Стрелки, Home/End, Esc, Delete, сочетания с Ctrl/Alt/Super — каретка ушла, контекст неизвестен.
    Reset,
    /// Клик мышью: как Reset, плюс следующее слово — возможно, середина чужого слова.
    Click,
    /// Сменилось активное окно.
    Focus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    None,
    Replace { delete: usize, text: String, layout_cyrillic: Option<bool> },
    /// Конвертировать нечего — просто переключить раскладку.
    ToggleLayout,
}

#[derive(Debug, Clone)]
struct Word {
    text: String,
    tail: String,
}

#[derive(Debug, Clone)]
struct LastAuto {
    index: usize,
    original: String,
    converted: String,
}

const MAX_WORDS: usize = 64;

pub struct Engine {
    cur: String,
    words: Vec<Word>,
    after_jump: bool,
    /// Текущее слово сконвертировано вручную — на пробеле авто его не трогает.
    cur_manual: bool,
    last_auto: Option<LastAuto>,
    pub auto: bool,
    pub excluded_app: bool,
    pub exc: Exceptions,
    pub log_text: bool,
}

fn n(s: &str) -> usize {
    s.chars().count()
}

impl Engine {
    pub fn new(exc: Exceptions) -> Self {
        Engine {
            cur: String::new(),
            words: Vec::new(),
            after_jump: false,
            cur_manual: false,
            last_auto: None,
            auto: true,
            excluded_app: false,
            exc,
            log_text: false,
        }
    }

    pub fn has_current_word(&self) -> bool {
        !self.cur.is_empty()
    }

    fn reset(&mut self) {
        self.cur.clear();
        self.words.clear();
        self.cur_manual = false;
        self.last_auto = None;
    }

    fn show<'a>(&self, s: &'a str) -> std::borrow::Cow<'a, str> {
        if self.log_text { s.into() } else { format!("<{} симв.>", n(s)).into() }
    }

    pub fn input(&mut self, ev: Input) -> Action {
        match ev {
            Input::Char(c) => {
                self.cur.push(c);
                Action::None
            }
            Input::Backspace { word: true } => {
                self.reset();
                Action::None
            }
            Input::Backspace { word: false } => {
                if self.cur.pop().is_none() {
                    if let Some(last) = self.words.last_mut() {
                        last.tail.pop();
                        if last.tail.is_empty() {
                            let w = self.words.pop().unwrap();
                            self.cur = w.text;
                            self.cur_manual = false;
                            if self.last_auto.as_ref().is_some_and(|a| a.index >= self.words.len()) {
                                self.last_auto = None;
                            }
                        }
                    }
                }
                Action::None
            }
            Input::Space => self.boundary(),
            Input::Break | Input::Reset | Input::Focus => {
                self.reset();
                self.after_jump = false;
                Action::None
            }
            Input::Click => {
                self.reset();
                self.after_jump = true;
                Action::None
            }
        }
    }

    fn push_word(&mut self, text: String, tail: &str) {
        self.words.push(Word { text, tail: tail.to_string() });
        if self.words.len() > MAX_WORDS {
            self.words.remove(0);
            if let Some(a) = self.last_auto.as_mut() {
                if a.index == 0 {
                    self.last_auto = None;
                } else {
                    a.index -= 1;
                }
            }
        }
    }

    fn boundary(&mut self) -> Action {
        if self.cur.is_empty() {
            if let Some(last) = self.words.last_mut() {
                last.tail.push(' ');
            }
            return Action::None;
        }
        let word = std::mem::take(&mut self.cur);
        let manual = std::mem::replace(&mut self.cur_manual, false);
        let after_jump = std::mem::replace(&mut self.after_jump, false);
        let len = self.words.len();
        let prev = len.checked_sub(1).map(|i| self.words[i].text.clone());
        let earlier = len.checked_sub(2).map(|i| self.words[i].text.clone());
        self.push_word(word.clone(), " ");

        if !self.auto || self.excluded_app || manual {
            return Action::None;
        }
        let (decision, rescue) = match detector::mixed_rescue(&word) {
            Decision::Keep => (detector::decide(&word, &self.exc, prev.as_deref(), earlier.as_deref(), after_jump), false),
            d => (d, true),
        };
        let Decision::Convert { to_cyrillic } = decision else {
            log::debug!("авто: оставляю {}", self.show(&word));
            return Action::None;
        };
        let converted = if rescue {
            keymap::convert(&word, to_cyrillic)
        } else {
            let ru = &crate::data::data().words_ru;
            let valid = |s: &str| ru.contains(s);
            keymap::smart_convert(&word, to_cyrillic, Some(&valid))
        };
        if converted == word {
            return Action::None;
        }
        log::info!("авто: {} → {} ({})", self.show(&word), self.show(&converted), if to_cyrillic { "RU" } else { "EN" });
        let index = self.words.len() - 1;
        self.words[index].text = converted.clone();
        self.last_auto = Some(LastAuto { index, original: word.clone(), converted: converted.clone() });
        Action::Replace { delete: n(&word) + 1, text: format!("{converted} "), layout_cyrillic: Some(to_cyrillic) }
    }

    fn direction(text: &str, current_cyrillic: bool) -> bool {
        if text.has_cyrillic() {
            false
        } else if text.has_latin_letter() {
            true
        } else {
            keymap::unambiguous_symbol_direction(text).unwrap_or(!current_cyrillic)
        }
    }

    /// Хоткей «слово». `current_cyrillic` — включена ли сейчас русская раскладка.
    pub fn manual_word(&mut self, current_cyrillic: bool) -> Action {
        if !self.cur.is_empty() {
            let to_cyr = Self::direction(&self.cur, current_cyrillic);
            let converted = keymap::convert(&self.cur, to_cyr);
            let delete = n(&self.cur);
            log::info!("хоткей: текущее слово {} → {}", self.show(&self.cur), self.show(&converted));
            self.cur = converted.clone();
            self.cur_manual = true;
            return Action::Replace { delete, text: converted, layout_cyrillic: Some(to_cyr) };
        }
        let Some(idx) = self.words.len().checked_sub(1) else {
            return Action::ToggleLayout;
        };
        let w = self.words[idx].clone();
        // Сразу после авто-исправления хоткей = «верни как было» + запомнить слово.
        if let Some(a) = self.last_auto.take() {
            if a.index == idx && a.converted == w.text {
                let key = detector::letter_core(&a.original).to_lowercase();
                log::info!("хоткей: отмена авто-исправления, слово {} в исключения", self.show(&key));
                if !key.is_empty() {
                    self.exc.learned.insert(key);
                    self.exc.save();
                }
                self.words[idx].text = a.original.clone();
                let to_cyr = a.original.has_cyrillic();
                return Action::Replace {
                    delete: n(&w.text) + n(&w.tail),
                    text: format!("{}{}", a.original, w.tail),
                    layout_cyrillic: Some(to_cyr),
                };
            }
        }
        let to_cyr = Self::direction(&w.text, current_cyrillic);
        let converted = keymap::convert(&w.text, to_cyr);
        log::info!("хоткей: последнее слово {} → {}", self.show(&w.text), self.show(&converted));
        self.words[idx].text = converted.clone();
        Action::Replace {
            delete: n(&w.text) + n(&w.tail),
            text: format!("{converted}{}", w.tail),
            layout_cyrillic: Some(to_cyr),
        }
    }

    /// Хоткей «фраза»: всё, что набрано с последнего сброса контекста.
    pub fn manual_phrase(&mut self, current_cyrillic: bool) -> Action {
        if self.words.is_empty() {
            return self.manual_word(current_cyrillic);
        }
        let mut whole = String::new();
        for w in &self.words {
            whole.push_str(&w.text);
            whole.push_str(&w.tail);
        }
        whole.push_str(&self.cur);
        // Направление — по последнему слову с буквами: его человек видит и хочет «перевернуть».
        let probe = std::iter::once(self.cur.as_str())
            .chain(self.words.iter().rev().map(|w| w.text.as_str()))
            .find(|t| t.has_cyrillic() || t.has_latin_letter())
            .unwrap_or("");
        let to_cyr = Self::direction(probe, current_cyrillic);
        for w in &mut self.words {
            w.text = keymap::convert(&w.text, to_cyr);
        }
        self.cur = keymap::convert(&self.cur, to_cyr);
        self.cur_manual = !self.cur.is_empty();
        self.last_auto = None;
        let converted = keymap::convert(&whole, to_cyr);
        log::info!("хоткей: фраза {} → {}", self.show(&whole), self.show(&converted));
        Action::Replace { delete: n(&whole), text: converted, layout_cyrillic: Some(to_cyr) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn typ(e: &mut Engine, s: &str) -> Vec<Action> {
        s.chars()
            .map(|c| if c == ' ' { e.input(Input::Space) } else { e.input(Input::Char(c)) })
            .filter(|a| *a != Action::None)
            .collect()
    }

    #[test]
    fn auto_fix_and_undo() {
        let mut e = Engine::new(Exceptions::default());
        let acts = typ(&mut e, "ghbdtn ");
        assert_eq!(
            acts,
            vec![Action::Replace { delete: 7, text: "привет ".into(), layout_cyrillic: Some(true) }]
        );
        // отмена хоткеем
        let undo = e.manual_word(true);
        assert_eq!(undo, Action::Replace { delete: 7, text: "ghbdtn ".into(), layout_cyrillic: Some(false) });
        assert!(e.exc.learned.contains("ghbdtn"));
    }

    #[test]
    fn context_and_manual() {
        let mut e = Engine::new(Exceptions::default());
        assert!(typ(&mut e, "hello world ").is_empty());
        assert_eq!(
            e.manual_word(false),
            Action::Replace { delete: 6, text: "цщкдв ".into(), layout_cyrillic: Some(true) }
        );
        typ(&mut e, "ghb");
        assert_eq!(e.manual_word(false), Action::Replace { delete: 3, text: "при".into(), layout_cyrillic: Some(true) });
        // ручное слово авто не переворачивает обратно
        assert!(typ(&mut e, " ").is_empty());
    }

    #[test]
    fn backspace_into_previous_word() {
        let mut e = Engine::new(Exceptions::default());
        typ(&mut e, "hello ");
        e.input(Input::Backspace { word: false });
        typ(&mut e, "x");
        assert_eq!(e.cur, "hellox");
    }

    #[test]
    fn phrase() {
        let mut e = Engine::new(Exceptions::default());
        e.auto = false;
        typ(&mut e, "ghbdtn vbh");
        assert_eq!(
            e.manual_phrase(false),
            Action::Replace { delete: 10, text: "привет мир".into(), layout_cyrillic: Some(true) }
        );
    }
}
