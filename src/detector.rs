//! Двусторонний детектор «слово набрано не в той раскладке». Порт `LayoutDetector.swift`
//! (метод `decide` и всё, что он использует). Подробные обоснования каждого правила —
//! в комментариях оригинала; здесь оставлены короткие пометки и номера задач для сверки.

use crate::data::{data, LayoutData};
use crate::exceptions::Exceptions;
use crate::keymap::{self, ScriptExt, EN_TO_RU, RU_TO_EN};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Keep,
    Convert { to_cyrillic: bool },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Context {
    None,
    Cyrillic,
    Latin,
}

impl Context {
    fn of(word: Option<&str>) -> Self {
        match word {
            Some(w) if !w.is_empty() => {
                if w.has_cyrillic() {
                    Context::Cyrillic
                } else if w.has_latin_letter() {
                    Context::Latin
                } else {
                    Context::None
                }
            }
            _ => Context::None,
        }
    }
}

const MARGIN: f64 = 2.0;
const SHORT_EN_SWAP_FLOOR: f64 = -10.0;

const FORCE_SWAP: &[&str] = &[
    "http", "https", "url", "uri", "api", "rest", "json", "xml", "yaml", "csv", "html", "css", "sdk", "cli",
    "gui", "ide", "ssh", "ftp", "tcp", "udp", "ip", "dns", "vpn", "ssl", "tls", "smtp", "jwt", "cors", "sql",
    "nosql", "git", "npm", "yarn", "k8s", "aws", "gcp", "kpi", "crm", "seo", "smm", "mvp", "cpu", "gpu", "ram",
    "ssd", "hdd", "usb", "pdf", "mp3", "mp4", "png", "jpg", "jpeg", "svg", "gif", "ddos", "iot", "llm", "gpt",
    "ml", "ai", "ui", "ux", "db", "os", "io", "qa", "ci", "cd", "webp", "mvc", "orm", "cdn", "dom",
];

const COMMON_EN_TWO_LETTER: &[&str] = &[
    "am", "an", "as", "at", "be", "by", "do", "go", "he", "hi", "if", "in", "is", "it", "me", "my", "no", "of",
    "ok", "on", "or", "so", "to", "up", "us", "we", "id", "ai", "ui", "ux",
];

const RU_SINGLE_LETTER: &[&str] = &["а", "в", "и", "к", "о", "с", "у", "я"];
const RU_VOWELS: &[char] = &['а', 'е', 'ё', 'и', 'о', 'у', 'ы', 'э', 'ю', 'я'];

fn is_force_swap(w: &str) -> bool {
    FORCE_SWAP.contains(&w)
}

fn lower(s: &str) -> String {
    s.to_lowercase()
}

/// Символ — буква, ИЛИ его клавиша в другой раскладке даёт букву (х=[, ж=;, э=', ё=`, ъ=]).
pub fn is_layout_letter(c: char) -> bool {
    if c.is_alphabetic() {
        return true;
    }
    if EN_TO_RU.get(&c).is_some_and(|m| m.is_alphabetic()) {
        return true;
    }
    RU_TO_EN.get(&c).is_some_and(|m| m.is_alphabetic())
}

/// Буквенное ядро: срезаем ведущие/концевые скобки, кавычки, пунктуацию (цифры оставляем).
pub fn letter_core(raw: &str) -> String {
    let keep = |c: char| is_layout_letter(c) || c.is_numeric();
    let s = raw.trim_start_matches(|c: char| !keep(c));
    s.trim_end_matches(|c: char| !keep(c)).to_string()
}

/// «круууто» → ["круто"], «класссно» → ["классно", "класно"]. None, если растягивать нечего.
pub fn de_elongated(s: &str) -> Option<Vec<String>> {
    let mut run = 0;
    let mut prev: Option<char> = None;
    let mut has = false;
    let (mut to_two, mut to_one) = (String::new(), String::new());
    for c in s.chars() {
        if Some(c) == prev {
            run += 1;
        } else {
            run = 1;
            prev = Some(c);
        }
        if run >= 3 {
            has = true;
        }
        if run <= 2 {
            to_two.push(c);
        }
        if run == 1 {
            to_one.push(c);
        }
    }
    if !has {
        return None;
    }
    Some(if to_two == to_one { vec![to_one] } else { vec![to_two, to_one] })
}

fn has_valid_source_before_trailing_punctuation(raw_core: &str, forced: &std::collections::HashSet<String>) -> bool {
    let d = data();
    let whole = lower(raw_core);
    let semantic = keymap::core_of(&whole);
    if semantic == whole || semantic.chars().count() < 2 {
        return false;
    }
    let cyr = semantic.has_cyrillic();
    let lat = semantic.has_latin_letter();
    if cyr == lat {
        return false;
    }
    if is_force_swap(&semantic) || forced.contains(&semantic) {
        return true;
    }
    if lat && d.extra.tech_latin_tokens.contains(&semantic) {
        return true;
    }
    if lat && d.extra.force_ru_amb.contains(&semantic) {
        return false;
    }
    let words = if cyr { &d.words_ru } else { &d.words_en };
    if words.contains(&semantic) {
        return true;
    }
    de_elongated(&semantic).is_some_and(|v| v.iter().any(|x| words.contains(x)))
}

/// Смайлик западного типа (`:D`, `;P`, `XD`) — никогда не трогаем (отзывы #247–248).
pub fn is_emoticon(raw: &str) -> bool {
    let mut s: Vec<char> = raw.chars().collect();
    while s.last().is_some_and(|l| ".,!?…".contains(*l)) {
        s.pop();
    }
    if s.is_empty() {
        return false;
    }
    let eyes = s.remove(0);
    if s.len() > 1 && "-~^'".contains(s[0]) {
        s.remove(0);
    }
    if s.len() != 1 {
        return false;
    }
    let mouth = s[0];
    let mouth_symbols = ")(|/\\*$@[]{}3";
    match eyes {
        ':' | '=' => mouth.is_alphabetic() || mouth_symbols.contains(mouth),
        ';' | '8' => (mouth.is_alphabetic() && mouth.is_uppercase()) || mouth_symbols.contains(mouth),
        'X' | 'x' => mouth == 'D' || mouth == 'd',
        _ => false,
    }
}

fn is_jcuken_swap(w: &str, swapped: &str) -> bool {
    let mut out = String::new();
    for ch in w.chars() {
        match RU_TO_EN.get(&ch) {
            Some(m) => out.push(*m),
            None => return false,
        }
    }
    lower(&out) == swapped
}

fn is_tech_token_from_cyrillic(w: &str, swapped: &str) -> bool {
    data().extra.tech_from_cyrillic.contains(swapped) && is_jcuken_swap(w, swapped)
}

/// Число слева: «5 г», «100 гб» (отзыв #294).
fn is_numeric_token(s: Option<&str>) -> bool {
    let Some(s) = s else { return false };
    if s.is_empty() {
        return false;
    }
    let mut has_digit = false;
    for ch in s.chars() {
        if ch.is_numeric() {
            has_digit = true;
            continue;
        }
        if ",.-–/".contains(ch) {
            continue;
        }
        return false;
    }
    has_digit
}

/// Кириллица, которой не бывает в русском (задача 258): «ЬФСИЩЩЛ» = MACBOOK.
fn is_impossible_russian_spelling(w: &str) -> bool {
    let chars: Vec<char> = lower(w).chars().collect();
    let distinct: std::collections::HashSet<_> = chars.iter().collect();
    if chars.len() < 2 || distinct.len() <= 1 {
        return false;
    }
    if chars[0] == 'ь' || chars[0] == 'ы' {
        return true;
    }
    for p in chars.windows(2) {
        let (a, b) = (p[0], p[1]);
        if a == b && "ьыйщ".contains(a) {
            return true;
        }
        if RU_VOWELS.contains(&a) && (b == 'ь' || b == 'ы') {
            return true;
        }
    }
    false
}

/// Форма слова через ASCII-дефис (`HyphenTokenShape.linguisticCore`).
fn hyphen_linguistic_core(raw: &str) -> Option<String> {
    let chars: Vec<char> = raw.chars().collect();
    let ok = |c: char| is_layout_letter(c) || c.is_numeric();
    let first = chars.iter().position(|&c| ok(c))?;
    let last = chars.iter().rposition(|&c| ok(c))?;
    if chars[..first].contains(&'-') || chars[last + 1..].contains(&'-') {
        return None;
    }
    let mut seg_len = 0;
    let mut segments = 0;
    for &c in &chars[first..=last] {
        if c == '-' {
            if seg_len == 0 {
                return None;
            }
            seg_len = 0;
            segments += 1;
        } else {
            if !is_layout_letter(c) {
                return None;
            }
            seg_len += 1;
        }
    }
    if segments < 1 || seg_len == 0 {
        return None;
    }
    Some(chars[first..=last].iter().collect())
}

/// Мост `… касается iOS, nj` (русская фраза с латинской аббревиатурой).
fn russian_nj_after_latin_label(word: &str, raw_core: &str, prev: Option<&str>, earlier: Option<&str>) -> bool {
    let (Some(prev), Some(earlier)) = (prev, earlier) else { return false };
    if word != "nj" || raw_core != word || !prev.ends_with(',') {
        return false;
    }
    if !earlier.has_cyrillic() || earlier.has_latin_letter() {
        return false;
    }
    let label = &prev[..prev.len() - 1];
    if label.is_empty() || !label.chars().all(|c| c.is_ascii_alphabetic()) {
        return false;
    }
    label.chars().filter(|c| c.is_uppercase()).count() >= 2
}

/// Смешанное слово (кир+лат): чиним, только если ровно одна сторона даёт словарное слово.
pub fn mixed_rescue(raw: &str) -> Decision {
    if !(raw.has_cyrillic() && raw.has_latin_letter()) {
        return Decision::Keep;
    }
    let core = letter_core(raw);
    if core.chars().count() < 2 || !core.chars().all(is_layout_letter) {
        return Decision::Keep;
    }
    let d = data();
    let to_ru = keymap::convert(&core, true);
    let to_en = keymap::convert(&core, false);
    let ru_ok = !to_ru.has_latin_letter() && d.words_ru.contains(&lower(&to_ru));
    let en_ok = !to_en.has_cyrillic() && d.words_en.contains(&lower(&to_en));
    match (ru_ok, en_ok) {
        (true, false) => Decision::Convert { to_cyrillic: true },
        (false, true) => Decision::Convert { to_cyrillic: false },
        _ => Decision::Keep,
    }
}

/// Решение для авто-режима на границе слова.
/// `prev`/`earlier` — предыдущие слова фразы (как на экране); `after_caret_jump` — первое слово
/// после клика мышью.
pub fn decide(raw: &str, exc: &Exceptions, prev: Option<&str>, earlier: Option<&str>, after_caret_jump: bool) -> Decision {
    use Decision::*;
    let d: &LayoutData = data();
    let x = &d.extra;
    let context = Context::of(prev);
    let letter_core = letter_core(raw);
    let literal = lower(&letter_core);
    let typed = lower(&keymap::core_of(&letter_core));
    let whole = lower(raw);
    for k in [&literal, &typed, &whole] {
        if exc.ignored.contains(k) || exc.learned.contains(k) {
            return Keep;
        }
    }
    if is_emoticon(raw) {
        return Keep;
    }

    let had_digits = letter_core.chars().any(|c| c.is_numeric());
    let core_raw: String = if had_digits {
        letter_core.trim_matches(|c: char| c.is_numeric()).to_string()
    } else {
        letter_core.clone()
    };
    let w = lower(&core_raw);
    let wlen = w.chars().count();

    // Дефисные слова: разбор по частям (задача от 21.08.2026).
    if w.contains('-') {
        let Some(hyphen_raw) = hyphen_linguistic_core(raw) else { return Keep };
        let hyphen_word = lower(&hyphen_raw);
        if hyphen_word != w {
            return Keep;
        }
        if hyphen_word.has_cyrillic() != hyphen_word.has_latin_letter() {
            let to_cyr = !hyphen_word.has_cyrillic();
            let swapped = lower(&keymap::convert(&hyphen_raw, to_cyr));
            if x.hyphen_terms.contains(&hyphen_word) || x.ru_hyphen_terms.contains(&hyphen_word) {
                return Keep;
            }
            if x.hyphen_terms.contains(&swapped) || x.ru_hyphen_terms.contains(&swapped) {
                return Convert { to_cyrillic: to_cyr };
            }
            let src: Vec<&str> = hyphen_word.split('-').collect();
            let dst: Vec<&str> = swapped.split('-').collect();
            if src.len() == dst.len()
                && src.iter().all(|s| s.chars().count() >= 2)
                && dst.iter().all(|s| s.chars().count() >= 2 && s.chars().all(|c| c.is_alphabetic()))
            {
                let latin = hyphen_word.has_latin_letter();
                let dict_src = if latin { &d.words_en } else { &d.words_ru };
                let dict_dst = if latin { &d.words_ru } else { &d.words_en };
                if src.iter().all(|s| dict_src.contains(*s)) {
                    return Keep;
                }
                if dst.iter().all(|s| dict_dst.contains(*s)) {
                    return Convert { to_cyrillic: to_cyr };
                }
            }
        }
        return Keep;
    }

    if wlen < (if had_digits { 4 } else { 1 }) || !w.chars().all(is_layout_letter) {
        return Keep;
    }
    if exc.ignored.contains(&w) || exc.learned.contains(&w) || x.default_keep.contains(&w) {
        return Keep;
    }
    let source_cyr = w.has_cyrillic();
    let source_lat = w.has_latin_letter();
    if source_cyr == source_lat {
        return Keep;
    }
    let to_cyr = !source_cyr;
    let swapped = lower(&keymap::convert(&core_raw, to_cyr));

    // Запятая/точка, набранные в русской раскладке, приезжают буквами «б»/«ю»/«ж» (отзыв #121).
    if source_cyr && !d.words_ru.contains(&w) && swapped.chars().last().is_some_and(keymap::is_trailing_punct) {
        let mut trimmed: Vec<char> = core_raw.chars().collect();
        while let Some(&l) = trimmed.last() {
            let mapped = keymap::convert(&l.to_string(), false);
            if mapped.chars().last().is_some_and(keymap::is_trailing_punct) {
                trimmed.pop();
            } else {
                break;
            }
        }
        if trimmed.is_empty() {
            return Keep;
        }
        let t: String = trimmed.into_iter().collect();
        return decide(&t, exc, prev, earlier, after_caret_jump);
    }
    if swapped == w || !swapped.chars().all(|c| c.is_alphabetic() || c == '\'') {
        return Keep;
    }

    // 1. Force-swap — только если исходник не настоящее слово.
    let source_is_real_word = has_valid_source_before_trailing_punctuation(&core_raw, &exc.force_swap)
        || d.words_ru.contains(&w)
        || d.words_en.contains(&w)
        || de_elongated(&w).is_some_and(|v| v.iter().any(|s| d.words_ru.contains(s) || d.words_en.contains(s)));
    if is_force_swap(&swapped) && !source_is_real_word {
        return Convert { to_cyrillic: to_cyr };
    }
    // «ече» → «txt», только отдельным словом (отзыв #310).
    if source_cyr && !source_is_real_word && !after_caret_jump && is_tech_token_from_cyrillic(&w, &swapped) {
        return Convert { to_cyrillic: false };
    }
    if exc.force_swap.contains(&swapped) {
        return Convert { to_cyrillic: to_cyr };
    }
    if is_force_swap(&w) || exc.force_swap.contains(&w) {
        return Keep;
    }
    if source_lat && x.tech_latin_tokens.contains(&w) {
        return Keep;
    }

    // ★ STRICT-GATE: слово, валидное в своём языке, не переключаем.
    if wlen >= 2 && source_is_real_word && !(source_lat && x.force_ru_amb.contains(&w)) {
        return Keep;
    }

    // Заглавная посреди фразы — это имя (отзыв #122).
    if prev.is_some() {
        let mut cs = core_raw.chars();
        if let Some(first) = cs.next() {
            if first.is_uppercase() && cs.any(|c| c.is_lowercase()) {
                return Keep;
            }
        }
    }

    // 2. Одиночные буквы.
    if wlen == 1 {
        if after_caret_jump && context == Context::None {
            return Keep;
        }
        if source_lat && RU_SINGLE_LETTER.contains(&swapped.as_str()) {
            if let Some(p) = prev {
                let p = lower(p);
                if context == Context::Latin && x.label_classifiers.contains(&p) {
                    return Keep;
                }
                if context == Context::Cyrillic && x.ru_label_classifiers.contains(&p) {
                    return Keep;
                }
            }
            return Convert { to_cyrillic: true };
        }
        if source_cyr && context != Context::Cyrillic && ["i", "u", "a"].contains(&swapped.as_str()) {
            if is_numeric_token(prev) {
                return Keep;
            }
            return Convert { to_cyrillic: false };
        }
        return Keep;
    }

    let en_swap_not_junk = || {
        if wlen == 2 {
            return COMMON_EN_TWO_LETTER.contains(&swapped.as_str());
        }
        wlen >= 4 || d.plausibility(&swapped, false) > SHORT_EN_SWAP_FLOOR
    };

    // 3. Словарь (коллизии разрешает контекст фразы).
    if source_lat {
        if d.words_en.contains(&w) && !x.force_ru_amb.contains(&w) {
            if context == Context::Cyrillic && d.words_ru.contains(&swapped) && !x.en_keep_short.contains(&w) {
                return Convert { to_cyrillic: true };
            }
            return Keep;
        }
        if d.words_ru.contains(&swapped) {
            if context == Context::Latin
                && x.en_keep_short.contains(&w)
                && !russian_nj_after_latin_label(&w, &core_raw, prev, earlier)
            {
                return Keep;
            }
            return Convert { to_cyrillic: true };
        }
    } else {
        if x.force_en_amb.contains(&swapped) && !d.words_ru.contains(&w) {
            return Convert { to_cyrillic: false };
        }
        if d.words_ru.contains(&w) {
            if context == Context::Latin && d.words_en.contains(&swapped) {
                return Convert { to_cyrillic: false };
            }
            return Keep;
        }
        if d.words_en.contains(&swapped) && en_swap_not_junk() {
            return Convert { to_cyrillic: false };
        }
    }

    // 4. Триграммы — только от 4 букв.
    if wlen < 4 {
        return Keep;
    }
    // Русская аббревиатура заглавными остаётся (задача 258), кроме невозможной орфографии.
    if source_cyr
        && !core_raw.chars().any(|c| c.is_lowercase())
        && core_raw.chars().filter(|c| c.is_alphabetic()).count() >= 2
        && !is_impossible_russian_spelling(&w)
    {
        return Keep;
    }
    let orig = d.plausibility(&w, source_cyr);
    let swap = d.plausibility(&swapped, to_cyr);
    if swap > orig + MARGIN {
        return Convert { to_cyrillic: to_cyr };
    }
    if orig <= -19.0 && swap > orig + 1.0 {
        return Convert { to_cyrillic: to_cyr };
    }
    Keep
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dec(w: &str, prev: Option<&str>) -> Decision {
        decide(w, &Exceptions::default(), prev, None, false)
    }
    const RU: Decision = Decision::Convert { to_cyrillic: true };
    const EN: Decision = Decision::Convert { to_cyrillic: false };
    const KEEP: Decision = Decision::Keep;

    #[test]
    fn cases() {
        let cases: &[(&str, Option<&str>, Decision)] = &[
            ("ghbdtn", None, RU),
            ("ghbdtn.", None, RU),
            ("руддщ", None, EN),
            ("hello", None, KEEP),
            ("привет", None, KEEP),
            ("her", None, KEEP),
            ("yt", Some("привет"), RU),
            ("in", Some("hello"), KEEP),
            ("d", None, RU),
            ("vitamin", None, KEEP),
            ("d", Some("vitamin"), KEEP),
            (":D", None, KEEP),
            ("УФНС", None, KEEP),
            ("ьфсищщл", None, EN),
            ("xnj-nj", None, RU),
            ("что-то", None, KEEP),
            ("ghbdtn2", None, RU),
            ("gj1", None, KEEP),
            ("ФЗШ", None, EN),
            ("круууто", None, KEEP),
            ("lf", None, RU),
            ("г", Some("5"), KEEP),
            ("Уфе", Some("по"), KEEP),
            ("лунищщзб", None, EN),
            ("rfr", None, RU),
            ("ns", None, RU),
        ];
        let mut bad = vec![];
        for (w, p, want) in cases {
            let got = dec(w, *p);
            if got != *want {
                bad.push(format!("{w:?} prev={p:?}: want {want:?}, got {got:?}"));
            }
        }
        assert!(bad.is_empty(), "\n{}", bad.join("\n"));
    }

    #[test]
    fn mixed() {
        assert_eq!(mixed_rescue("привtn"), RU);
        assert_eq!(mixed_rescue("helloмир"), KEEP);
    }
}
