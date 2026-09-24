use std::borrow::Cow;
use std::cmp::Ordering;
use std::future::Future;

use crate::error::PikeError;

pub fn truncate_str(s: &str, max: usize) -> Cow<'_, str> {
    if max == 0 {
        return Cow::Borrowed("");
    }
    if s.chars().count() <= max {
        return Cow::Borrowed(s);
    }
    let end = s
        .char_indices()
        .nth(max.saturating_sub(1))
        .map_or(s.len(), |(i, _)| i);
    Cow::Owned(format!("{}…", &s[..end]))
}

pub async fn gather<T, Fut>(futures: Vec<Fut>, label: &str) -> Vec<T>
where
    Fut: Future<Output = Result<Vec<T>, PikeError>>,
{
    let results = futures::future::join_all(futures).await;
    let mut items = Vec::new();
    for result in results {
        match result {
            Ok(v) => items.extend(v),
            Err(e) => {
                tracing::warn!("{} failed: {}", label, e);
            }
        }
    }
    items
}

pub fn sort_by_source<T>(
    items: &mut [T],
    source_of: impl Fn(&T) -> &crate::package::SourceType,
    key_of: impl Fn(&T) -> &str,
) {
    items.sort_by(|a, b| {
        source_of(a)
            .cmp(source_of(b))
            .then_with(|| key_of(a).cmp(key_of(b)))
    });
}

pub fn filter_and_sort_packages(
    packages: &mut Vec<crate::package::Package>,
    config: &crate::config::Config,
) {
    packages.retain(|p| match &p.arch {
        Some(arch) => config.display.architectures.arch_allowed(arch, p.source),
        None => true,
    });
    sort_by_source(packages, |p| &p.source, |p| &p.name);
}

/// rpmvercmp semantics: `~` sorts before the end of string, `^` after it.
pub(crate) fn compare_versions(a: &str, b: &str) -> Ordering {
    let (mut a, mut b) = (a.as_bytes(), b.as_bytes());
    loop {
        a = skip_separators(a);
        b = skip_separators(b);
        match (a.first().copied(), b.first().copied()) {
            (Some(b'~'), Some(b'~')) | (Some(b'^'), Some(b'^')) => {
                a = &a[1..];
                b = &b[1..];
                continue;
            }
            (Some(b'~'), _) | (None, Some(b'^')) => return Ordering::Less,
            (_, Some(b'~')) | (Some(b'^'), None) => return Ordering::Greater,
            (Some(b'^'), _) => return Ordering::Less,
            (_, Some(b'^')) => return Ordering::Greater,
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(_), Some(_)) => {}
        }
        let numeric = a[0].is_ascii_digit();
        let (x, rest_a) = split_segment(a, numeric);
        let (y, rest_b) = split_segment(b, numeric);
        if y.is_empty() {
            return if numeric {
                Ordering::Greater
            } else {
                Ordering::Less
            };
        }
        let ord = if numeric {
            let x = trim_leading_zeros(x);
            let y = trim_leading_zeros(y);
            x.len().cmp(&y.len()).then_with(|| x.cmp(y))
        } else {
            x.cmp(y)
        };
        if ord != Ordering::Equal {
            return ord;
        }
        a = rest_a;
        b = rest_b;
    }
}

fn skip_separators(s: &[u8]) -> &[u8] {
    let n = s
        .iter()
        .take_while(|c| !c.is_ascii_alphanumeric() && **c != b'~' && **c != b'^')
        .count();
    &s[n..]
}

fn split_segment(s: &[u8], numeric: bool) -> (&[u8], &[u8]) {
    let n = s
        .iter()
        .take_while(|c| {
            if numeric {
                c.is_ascii_digit()
            } else {
                c.is_ascii_alphabetic()
            }
        })
        .count();
    s.split_at(n)
}

fn trim_leading_zeros(s: &[u8]) -> &[u8] {
    let n = s.iter().take_while(|c| **c == b'0').count();
    &s[n..]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cmp::Ordering;

    #[test]
    fn test_compare_versions_numeric_segments() {
        assert_eq!(
            compare_versions("7.2.10-200.fc44.x86_64", "7.2.9-200.fc44.x86_64"),
            Ordering::Greater
        );
        assert_eq!(
            compare_versions("6.8.0-100-generic", "6.8.0-45-generic"),
            Ordering::Greater
        );
        assert_eq!(
            compare_versions("7.2.5-200.fc44.x86_64", "7.2.5-200.fc44.x86_64"),
            Ordering::Equal
        );
        assert_eq!(compare_versions("7.2.5", "7.2.5.1"), Ordering::Less);
        assert_eq!(compare_versions("1.0a", "1.0"), Ordering::Greater);
    }

    #[test]
    fn test_compare_versions_tilde_caret() {
        assert_eq!(compare_versions("1.0~rc1", "1.0"), Ordering::Less);
        assert_eq!(compare_versions("1.0~rc1", "1.0~rc2"), Ordering::Less);
        assert_eq!(compare_versions("1.0", "1.0~rc1"), Ordering::Greater);
        assert_eq!(compare_versions("1.0^git1", "1.0"), Ordering::Greater);
        assert_eq!(compare_versions("1.0^git1", "1.0.1"), Ordering::Less);
        assert_eq!(compare_versions("1.0^git1", "1.0^git2"), Ordering::Less);
    }

    #[test]
    fn test_compare_versions_leading_zeros_and_debian() {
        assert_eq!(compare_versions("1.01", "1.1"), Ordering::Equal);
        assert_eq!(
            compare_versions("6.1.0-25-amd64", "6.1.0-3-amd64"),
            Ordering::Greater
        );
    }
}
