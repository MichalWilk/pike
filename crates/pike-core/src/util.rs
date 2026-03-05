use std::borrow::Cow;
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
