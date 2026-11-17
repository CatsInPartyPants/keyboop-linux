//! Соответствие физических клавиш латинской и русской раскладок. Порт `Keymap.swift`.
//!
//! Две таблицы, как в оригинале:
//! * статическая (US ↔ ЙЦУКЕН) — запаска и основа для `is_layout_letter`/`is_jcuken_swap`;
//! * живая — строится из реальной XKB-раскладки (аналог `DynamicKeymap`), см. `set_live_tables`.

use std::collections::HashMap;
use std::sync::{LazyLock, RwLock};

pub type Table = HashMap<char, char>;

const BASE_PAIRS: &[(char, char)] = &[
    ('`', 'ё'), ('q', 'й'), ('w', 'ц'), ('e', 'у'), ('r', 'к'), ('t', 'е'), ('y', 'н'), ('u', 'г'),
    ('i', 'ш'), ('o', 'щ'), ('p', 'з'), ('[', 'х'), (']', 'ъ'),
    ('a', 'ф'), ('s', 'ы'), ('d', 'в'), ('f', 'а'), ('g', 'п'), ('h', 'р'), ('j', 'о'), ('k', 'л'),
    ('l', 'д'), (';', 'ж'), ('\'', 'э'),
    ('z', 'я'), ('x', 'ч'), ('c', 'с'), ('v', 'м'), ('b', 'и'), ('n', 'т'), ('m', 'ь'),
    (',', 'б'), ('.', 'ю'), ('/', '.'),
];

fn upper(c: char) -> char {
    c.to_uppercase().next().unwrap_or(c)
}

fn is_cased_letter(c: char) -> bool {
    c.to_lowercase().ne(c.to_uppercase())
}

pub static EN_TO_RU: LazyLock<Table> = LazyLock::new(|| {
    let mut d = Table::new();
    for &(e, r) in BASE_PAIRS {
        d.insert(e, r);
        if is_cased_letter(e) {
            d.insert(upper(e), upper(r));
        }
    }
    d
});

pub static RU_TO_EN: LazyLock<Table> = LazyLock::new(|| {
    let mut d = Table::new();
    for &(e, r) in BASE_PAIRS {
        d.insert(r, e);
        if is_cased_letter(e) {
            d.insert(upper(r), upper(e));
        }
    }
    d
});

struct Live {
    en_to_ru: Table,
    ru_to_en: Table,
}

static LIVE: RwLock<Option<Live>> = RwLock::new(None);

/// Ставит живые таблицы, снятые с XKB-раскладки. Пустые → живых таблиц нет (запаска).
pub fn set_live_tables(en_to_ru: Table, ru_to_en: Table) {
    let v = if en_to_ru.is_empty() { None } else { Some(Live { en_to_ru, ru_to_en }) };
    *LIVE.write().unwrap() = v;
}

/// Копия живой таблицы нужного направления (или None). Таблицы маленькие (~100 пар).
pub fn live_table(to_cyrillic: bool) -> Option<Table> {
    LIVE.read()
        .unwrap()
        .as_ref()
        .map(|l| if to_cyrillic { l.en_to_ru.clone() } else { l.ru_to_en.clone() })
}

pub fn live_tables() -> Option<(Table, Table)> {
    LIVE.read().unwrap().as_ref().map(|l| (l.en_to_ru.clone(), l.ru_to_en.clone()))
}

/// Типографский двойник → прямой знак.
pub fn straighten(ch: char) -> char {
    match ch {
        '\u{2019}' | '\u{2018}' | '\u{00B4}' | '\u{2032}' => '\'',
        '\u{201C}' | '\u{201D}' | '\u{201F}' => '"',
        _ => ch,
    }
}

pub const TRAILING_PUNCTUATION: &[char] = &['.', ',', '!', '?', ';', ':', '…'];

pub fn is_trailing_punct(c: char) -> bool {
    TRAILING_PUNCTUATION.contains(&c)
}

fn is_ascii_digit(c: char) -> bool {
    c.is_ascii_digit()
}

pub fn convert(text: &str, to_cyrillic: bool) -> String {
    convert_with(text, to_cyrillic, live_table(to_cyrillic).as_ref())
}

/// Чистое ядро конверсии. Правило про знак между цифрами — см. комментарии в Keymap.swift (задача 263).
pub fn convert_with(text: &str, to_cyrillic: bool, table: Option<&Table>) -> String {
    let fallback: &Table = if to_cyrillic { &EN_TO_RU } else { &RU_TO_EN };
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len() * 2);
    let mut run_has_separator = false;
    for i in 0..chars.len() {
        let ch = chars[i];
        let prev = if i > 0 { Some(chars[i - 1]) } else { None };
        let next = chars.get(i + 1).copied();
        if (ch == ',' || ch == '.')
            && prev.is_some_and(is_ascii_digit)
            && next.is_some_and(is_ascii_digit)
        {
            out.push(number_separator(ch, to_cyrillic, run_has_separator, &chars[i + 2..]));
            run_has_separator = true;
        } else {
            let mapped = match table {
                Some(t) => t.get(&ch).or_else(|| t.get(&straighten(ch))).copied().unwrap_or(ch),
                None => fallback.get(&ch).or_else(|| fallback.get(&straighten(ch))).copied().unwrap_or(ch),
            };
            out.push(mapped);
            if !is_ascii_digit(ch) {
                run_has_separator = false;
            }
        }
    }
    out
}

/// `rest` начинается сразу за цифрой после знака.
fn number_separator(ch: char, to_cyrillic: bool, left_has_separator: bool, rest: &[char]) -> char {
    if to_cyrillic || ch == '.' || left_has_separator {
        return ch;
    }
    let mut i = 0;
    while i < rest.len() {
        let c = rest[i];
        if is_ascii_digit(c) {
            i += 1;
            continue;
        }
        if (c == ',' || c == '.') && rest.get(i + 1).is_some_and(|a| is_ascii_digit(*a)) {
            return ch;
        }
        break;
    }
    '.'
}

/// Направление ручной конверсии строки без букв (только однозначные знаки). Только живая таблица.
pub fn unambiguous_symbol_direction(text: &str) -> Option<bool> {
    let (en, ru) = live_tables()?;
    symbol_direction(text, &en, &ru)
}

pub fn symbol_direction(text: &str, en_to_ru: &Table, ru_to_en: &Table) -> Option<bool> {
    let (mut to_cyr, mut to_lat) = (false, false);
    for ch in text.chars() {
        if ch.is_alphabetic() {
            return None;
        }
        if ch.is_whitespace() || ch.is_numeric() {
            continue;
        }
        let c = straighten(ch);
        let en = en_to_ru.contains_key(&c);
        let ru = ru_to_en.contains_key(&c);
        if en && !ru {
            to_cyr = true;
        } else if ru && !en {
            to_lat = true;
        }
    }
    if to_cyr == to_lat { None } else { Some(to_cyr) }
}

/// Умная конвертация: концевую пунктуацию сохраняем как знак, если полное слово с ней как с
/// буквой не даёт словарного слова. `ghbdtn.` → `привет.`, но `yj;` → `нож`.
pub fn smart_convert(word: &str, to_cyrillic: bool, is_valid_target: Option<&dyn Fn(&str) -> bool>) -> String {
    smart_convert_with(word, to_cyrillic, is_valid_target, live_table(to_cyrillic).as_ref())
}

pub fn smart_convert_with(
    word: &str,
    to_cyrillic: bool,
    is_valid_target: Option<&dyn Fn(&str) -> bool>,
    table: Option<&Table>,
) -> String {
    if to_cyrillic {
        if let Some(valid) = is_valid_target {
            let full = convert_with(word, true, table);
            if valid(&full.to_lowercase()) {
                return full;
            }
        }
    }
    let mut core: Vec<char> = word.chars().collect();
    let mut trailing: Vec<char> = Vec::new();
    while let Some(&last) = core.last() {
        if !is_trailing_punct(last) {
            break;
        }
        trailing.insert(0, peeled_mark(last, to_cyrillic, table));
        core.pop();
    }
    if core.is_empty() {
        return word.to_string();
    }
    let core: String = core.into_iter().collect();
    let mut out = convert_with(&core, to_cyrillic, table);
    out.extend(trailing);
    out
}

fn peeled_mark(p: char, to_cyrillic: bool, table: Option<&Table>) -> char {
    if !to_cyrillic {
        return p;
    }
    let mapped = convert_with(&p.to_string(), true, table);
    let mut it = mapped.chars();
    match (it.next(), it.next()) {
        (Some(m), None) if m != p && is_trailing_punct(m) => m,
        _ => p,
    }
}

/// Ядро слова без концевой пунктуации.
pub fn core_of(word: &str) -> String {
    word.trim_end_matches(TRAILING_PUNCTUATION).to_string()
}

pub trait ScriptExt {
    fn has_cyrillic(&self) -> bool;
    fn has_latin_letter(&self) -> bool;
}

impl ScriptExt for str {
    fn has_cyrillic(&self) -> bool {
        self.chars().any(|c| ('\u{0400}'..='\u{04FF}').contains(&c))
    }
    fn has_latin_letter(&self) -> bool {
        self.chars().any(|c| c.is_ascii_alphabetic())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic() {
        assert_eq!(convert_with("ghbdtn", true, None), "привет");
        assert_eq!(convert_with("Ghbdtn", true, None), "Привет");
        assert_eq!(convert_with("руддщ", false, None), "hello");
        assert_eq!(smart_convert_with("ghbdtn.", true, None, None), "привет.");
        assert_eq!(convert_with("5,5", false, None), "5.5");
        assert_eq!(convert_with("1,000,000", false, None), "1,000,000");
        assert_eq!(convert_with("17.2", true, None), "17.2");
    }
}
