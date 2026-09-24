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

/// Search results: architecture filter, then relevance to `query`, source and name.
/// Relevance: exact name, name prefix, name contains, description contains. For flatpak
/// the display name counts as a name and the last app ID segment as a prefix match.
pub fn filter_and_sort_packages(
    packages: &mut Vec<crate::package::Package>,
    config: &crate::config::Config,
    query: &str,
) {
    packages.retain(|p| match &p.arch {
        Some(arch) => config.display.architectures.arch_allowed(arch, p.source),
        None => true,
    });
    let query = query.trim().to_lowercase();
    sort_by_source(packages, |p| &p.source, |p| &p.name);
    packages.sort_by_cached_key(|p| search_rank(p, &query));
}

fn search_rank(package: &crate::package::Package, query: &str) -> u8 {
    let name = package.name.to_lowercase();
    let mut rank = text_rank(&name, query);
    if package.source == crate::package::SourceType::Flatpak
        && let Some(short) = name.rsplit('.').next()
    {
        rank = rank.min(text_rank(short, query).max(1));
    }
    if let Some(display) = &package.display_name {
        rank = rank.min(text_rank(&display.to_lowercase(), query));
    }
    if rank > 3
        && package
            .description
            .as_deref()
            .is_some_and(|d| d.to_lowercase().contains(query))
    {
        rank = 3;
    }
    rank
}

fn text_rank(text: &str, query: &str) -> u8 {
    if text == query {
        0
    } else if text.starts_with(query) {
        1
    } else if text.contains(query) {
        2
    } else {
        4
    }
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

    fn pkg(name: &str, source: crate::package::SourceType, desc: &str) -> crate::package::Package {
        crate::package::Package {
            name: name.into(),
            display_name: None,
            version: String::new(),
            source,
            arch: None,
            description: Some(desc.into()),
        }
    }

    #[test]
    fn test_search_rank_orders_exact_prefix_contains_description() {
        use crate::package::SourceType::{Dnf, Flatpak};
        let mut firefox_flatpak = pkg("org.mozilla.firefox", Flatpak, "Web browser");
        firefox_flatpak.display_name = Some("Firefox".into());
        let mut packages = vec![
            pkg("cargo-firefox-marionette", Dnf, "Marionette client"),
            pkg("org.mozilla.firefox.BaseApp", Flatpak, "Base app"),
            pkg("firefox-langpacks", Dnf, "Language packs"),
            pkg("libgtk", Dnf, "Used by Firefox and others"),
            firefox_flatpak,
            pkg("firefox", Dnf, "Mozilla Firefox Web browser"),
            pkg("zlib", Dnf, "Compression"),
        ];
        filter_and_sort_packages(
            &mut packages,
            &crate::config::Config::default(),
            " FireFox ",
        );
        let names: Vec<&str> = packages.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "firefox",
                "org.mozilla.firefox",
                "firefox-langpacks",
                "cargo-firefox-marionette",
                "org.mozilla.firefox.BaseApp",
                "libgtk",
                "zlib",
            ]
        );
    }

    #[test]
    fn test_search_rank_full_name_for_non_flatpak() {
        use crate::package::SourceType::{Dnf, Flatpak};
        let mut packages = vec![
            pkg("python3.12", Dnf, "Python"),
            pkg("org.example.12", Flatpak, "Example"),
        ];
        filter_and_sort_packages(&mut packages, &crate::config::Config::default(), "12");
        let names: Vec<&str> = packages.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["org.example.12", "python3.12"]);
        assert_eq!(search_rank(&packages[1], "12"), 2);
    }

    #[test]
    fn test_search_rank_display_name() {
        use crate::package::SourceType::{Dnf, Flatpak};
        let mut spotify = pkg(
            "com.spotify.Client",
            Flatpak,
            "Online music streaming service",
        );
        spotify.display_name = Some("Spotify".into());
        assert_eq!(search_rank(&spotify, "spotify"), 0);
        let mut packages = vec![pkg("lpf-spotify-client", Dnf, "Spotify client"), spotify];
        filter_and_sort_packages(&mut packages, &crate::config::Config::default(), "spotify");
        let names: Vec<&str> = packages.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["com.spotify.Client", "lpf-spotify-client"]);
    }

    #[test]
    fn test_search_rank_last_segment_is_prefix_tier() {
        use crate::package::SourceType::Flatpak;
        let client = pkg("com.dropbox.Client", Flatpak, "Dropbox");
        assert_eq!(search_rank(&client, "client"), 1);
        assert_eq!(search_rank(&client, "dropbox"), 2);
        let exact = pkg("com.dropbox.client", Flatpak, "Dropbox");
        assert_eq!(search_rank(&exact, "com.dropbox.client"), 0);
    }

    #[test]
    fn test_search_empty_query_keeps_source_name_order() {
        use crate::package::SourceType::{Apt, Dnf, Flatpak};
        let mut packages = vec![
            pkg("zlib", Apt, "Compression"),
            pkg("org.b.App", Flatpak, "B"),
            pkg("vim", Dnf, "Editor"),
            pkg("bash", Dnf, "Shell"),
        ];
        filter_and_sort_packages(&mut packages, &crate::config::Config::default(), "  ");
        let names: Vec<&str> = packages.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["bash", "vim", "org.b.App", "zlib"]);
    }

    #[test]
    fn test_search_rank_without_description() {
        use crate::package::SourceType::Dnf;
        let mut package = pkg("libfoo", Dnf, "");
        package.description = None;
        assert_eq!(search_rank(&package, "foo"), 2);
        assert_eq!(search_rank(&package, "bar"), 4);
    }

    #[test]
    fn test_search_equal_rank_ties_by_source_then_name() {
        use crate::package::SourceType::{Apt, Dnf, Flatpak};
        let mut packages = vec![
            pkg("vim-b", Apt, "Editor"),
            pkg("org.vim.Vim-x", Flatpak, "Editor"),
            pkg("vim-z", Dnf, "Editor"),
            pkg("vim-a", Dnf, "Editor"),
            pkg("vim-a", Apt, "Editor"),
        ];
        filter_and_sort_packages(&mut packages, &crate::config::Config::default(), "vim");
        let names: Vec<(crate::package::SourceType, &str)> = packages
            .iter()
            .map(|p| (p.source, p.name.as_str()))
            .collect();
        assert_eq!(
            names,
            vec![
                (Dnf, "vim-a"),
                (Dnf, "vim-z"),
                (Flatpak, "org.vim.Vim-x"),
                (Apt, "vim-a"),
                (Apt, "vim-b"),
            ]
        );
    }
}
