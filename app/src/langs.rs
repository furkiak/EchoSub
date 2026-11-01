/// Whisper'ın desteklediği diller: (ISO kodu, whisper'ın tam adı, küçük harf).
pub const LANGS: &[(&str, &str)] = &[
    ("en", "english"), ("zh", "chinese"), ("de", "german"), ("es", "spanish"),
    ("ru", "russian"), ("ko", "korean"), ("fr", "french"), ("ja", "japanese"),
    ("pt", "portuguese"), ("tr", "turkish"), ("pl", "polish"), ("ca", "catalan"),
    ("nl", "dutch"), ("ar", "arabic"), ("sv", "swedish"), ("it", "italian"),
    ("id", "indonesian"), ("hi", "hindi"), ("fi", "finnish"), ("vi", "vietnamese"),
    ("he", "hebrew"), ("uk", "ukrainian"), ("el", "greek"), ("ms", "malay"),
    ("cs", "czech"), ("ro", "romanian"), ("da", "danish"), ("hu", "hungarian"),
    ("ta", "tamil"), ("no", "norwegian"), ("th", "thai"), ("ur", "urdu"),
    ("hr", "croatian"), ("bg", "bulgarian"), ("lt", "lithuanian"), ("lv", "latvian"),
    ("sk", "slovak"), ("sl", "slovenian"), ("sr", "serbian"), ("et", "estonian"),
    ("fa", "persian"), ("bn", "bengali"), ("az", "azerbaijani"), ("kk", "kazakh"),
    ("sq", "albanian"), ("mk", "macedonian"), ("is", "icelandic"), ("hy", "armenian"),
    ("ka", "georgian"), ("be", "belarusian"), ("uz", "uzbek"), ("sw", "swahili"),
    ("af", "afrikaans"), ("tl", "tagalog"), ("ne", "nepali"), ("mr", "marathi"),
    ("gu", "gujarati"), ("kn", "kannada"), ("te", "telugu"), ("ml", "malayalam"),
    ("pa", "punjabi"), ("mn", "mongolian"), ("yue", "cantonese"),
];

pub fn code_from_name(name: &str) -> Option<&'static str> {
    let n = name.trim().to_lowercase();
    LANGS.iter().find(|(_, nm)| *nm == n).map(|(c, _)| *c)
}

/// İngilizce, baş harfi büyük dil adı (çeviri istemi için).
pub fn display_name(code: &str) -> String {
    let nm = LANGS
        .iter()
        .find(|(c, _)| *c == code)
        .map(|(_, n)| *n)
        .unwrap_or(code);
    let mut ch = nm.chars();
    match ch.next() {
        Some(f) => f.to_uppercase().collect::<String>() + ch.as_str(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_and_names_are_unique() {
        let mut c: Vec<_> = LANGS.iter().map(|l| l.0).collect();
        let mut n: Vec<_> = LANGS.iter().map(|l| l.1).collect();
        c.sort_unstable();
        n.sort_unstable();
        let (lc, ln) = (c.len(), n.len());
        c.dedup();
        n.dedup();
        assert_eq!((c.len(), n.len()), (lc, ln));
    }

    #[test]
    fn name_lookup_is_case_insensitive() {
        assert_eq!(code_from_name("Japanese"), Some("ja"));
        assert_eq!(code_from_name("  turkish "), Some("tr"));
        assert_eq!(code_from_name("klingon"), None);
    }

    #[test]
    fn display_name_is_capitalised_english() {
        assert_eq!(display_name("ru"), "Russian");
        assert_eq!(display_name("yue"), "Cantonese");
        assert_eq!(display_name("xx"), "Xx", "bilinmeyen kod olduğu gibi döner");
    }
}
