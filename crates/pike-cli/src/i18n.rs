pub(crate) fn plural_key(base: &str, count: usize) -> String {
    let locale = rust_i18n::locale();
    let suffix = if locale.starts_with("pl") {
        polish_plural(count)
    } else {
        english_plural(count)
    };
    format!("{base}_{suffix}")
}

fn english_plural(n: usize) -> &'static str {
    if n == 1 { "one" } else { "other" }
}

fn polish_plural(n: usize) -> &'static str {
    if n == 1 {
        "one"
    } else if n % 10 >= 2 && n % 10 <= 4 && (n % 100 < 12 || n % 100 > 14) {
        "few"
    } else {
        "many"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_english_plural() {
        assert_eq!(english_plural(0), "other");
        assert_eq!(english_plural(1), "one");
        assert_eq!(english_plural(2), "other");
        assert_eq!(english_plural(100), "other");
    }

    #[test]
    fn test_polish_plural() {
        assert_eq!(polish_plural(1), "one");
        assert_eq!(polish_plural(2), "few");
        assert_eq!(polish_plural(3), "few");
        assert_eq!(polish_plural(4), "few");
        assert_eq!(polish_plural(5), "many");
        assert_eq!(polish_plural(11), "many");
        assert_eq!(polish_plural(12), "many");
        assert_eq!(polish_plural(13), "many");
        assert_eq!(polish_plural(14), "many");
        assert_eq!(polish_plural(21), "many");
        assert_eq!(polish_plural(22), "few");
        assert_eq!(polish_plural(23), "few");
        assert_eq!(polish_plural(24), "few");
        assert_eq!(polish_plural(25), "many");
        assert_eq!(polish_plural(100), "many");
        assert_eq!(polish_plural(101), "many");
        assert_eq!(polish_plural(102), "few");
        assert_eq!(polish_plural(112), "many");
    }
}
