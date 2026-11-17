//! Языковые данные: триграммы и словари RU/EN, плюс ручные списки из `ExtraWords.swift`.
//! Всё встроено в бинарник (include_str!), чтобы не зависеть от путей установки.
//! Источник триграмм/словарей — keyswitcher (MIT, © 2026 Ilya Granin), см. THIRD_PARTY.md в
//! основном репозитории.

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

pub type WordSet = HashSet<String>;

pub struct ExtraWords {
    pub ru_abbr: WordSet,
    pub ru_loan_names: WordSet,
    pub ru_short: WordSet,
    pub ru_common_forms: WordSet,
    pub force_ru_amb: WordSet,
    pub force_en_amb: WordSet,
    pub tech_latin_tokens: WordSet,
    pub tech_from_cyrillic: WordSet,
    pub default_keep: WordSet,
    pub hyphen_terms: WordSet,
    pub ru_hyphen_terms: WordSet,
    pub ru_label_classifiers: WordSet,
    pub en_keep_short: WordSet,
    pub label_classifiers: WordSet,
    pub ru_dev: WordSet,
    pub ru: WordSet,
    pub en: WordSet,
}

pub struct LayoutData {
    pub trigrams_ru: HashMap<String, f64>,
    pub trigrams_en: HashMap<String, f64>,
    pub words_ru: WordSet,
    pub words_en: WordSet,
    pub extra: ExtraWords,
}

static DATA: OnceLock<LayoutData> = OnceLock::new();

/// Данные грузятся один раз (~0.3 с) — вызывать `data()` заранее на старте, не на горячем пути.
pub fn data() -> &'static LayoutData {
    DATA.get_or_init(LayoutData::load)
}

fn set(v: Vec<String>) -> WordSet {
    v.into_iter().collect()
}

impl LayoutData {
    fn load() -> Self {
        let mut extra_raw: HashMap<String, Vec<String>> =
            serde_json::from_str(include_str!("../data/extra_words.json")).expect("extra_words.json");
        let mut take = |k: &str| set(extra_raw.remove(k).unwrap_or_else(|| panic!("нет набора {k}")));
        let extra = ExtraWords {
            ru_abbr: take("ruAbbr"),
            ru_loan_names: take("ruLoanNames"),
            ru_short: take("ruShort"),
            ru_common_forms: take("ruCommonForms"),
            force_ru_amb: take("forceRuAmb"),
            force_en_amb: take("forceEnAmb"),
            tech_latin_tokens: take("techLatinTokens"),
            tech_from_cyrillic: take("techFromCyrillic"),
            default_keep: take("defaultKeep"),
            hyphen_terms: take("hyphenTerms"),
            ru_hyphen_terms: take("ruHyphenTerms"),
            ru_label_classifiers: take("ruLabelClassifiers"),
            en_keep_short: take("enKeepShort"),
            label_classifiers: take("labelClassifiers"),
            ru_dev: take("ruDev"),
            ru: take("ru"),
            en: take("en"),
        };
        let trigrams_ru = serde_json::from_str(include_str!("../data/trigrams_ru.json")).expect("trigrams_ru");
        let trigrams_en = serde_json::from_str(include_str!("../data/trigrams_en.json")).expect("trigrams_en");
        let mut words_ru: WordSet =
            set(serde_json::from_str(include_str!("../data/words_ru.json")).expect("words_ru"));
        // Тот же состав, что в LayoutData.swift.
        for s in [&extra.ru, &extra.ru_dev, &extra.ru_abbr, &extra.ru_short, &extra.ru_common_forms, &extra.ru_loan_names] {
            words_ru.extend(s.iter().cloned());
        }
        let mut words_en: WordSet =
            set(serde_json::from_str(include_str!("../data/words_en.json")).expect("words_en"));
        words_en.extend(extra.en.iter().cloned());
        LayoutData { trigrams_ru, trigrams_en, words_ru, words_en, extra }
    }

    /// Средняя лог-вероятность триграмм слова (с паддингом пробелами). Штраф −20 за отсутствие.
    pub fn plausibility(&self, word: &str, cyrillic: bool) -> f64 {
        let table = if cyrillic { &self.trigrams_ru } else { &self.trigrams_en };
        let chars: Vec<char> = std::iter::once(' ')
            .chain(word.to_lowercase().chars())
            .chain(std::iter::once(' '))
            .collect();
        if chars.len() < 3 {
            return f64::NEG_INFINITY;
        }
        let mut sum = 0.0;
        let mut key = String::with_capacity(12);
        for w in chars.windows(3) {
            key.clear();
            key.extend(w.iter());
            sum += table.get(&key).copied().unwrap_or(-20.0);
        }
        sum / (chars.len() - 2) as f64
    }
}
