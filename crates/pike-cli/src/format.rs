use pike_core::cleanup::total_size;
use pike_core::error::PikeError;
use pike_core::package::{CleanupItem, SourceType};
use rust_i18n::t;

pub(crate) fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KiB", "MiB", "GiB", "TiB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64 / 1024.0;
    let mut unit = 0;
    while (value * 10.0).round() >= 10240.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

pub(crate) fn items_summary(items: &[CleanupItem]) -> String {
    t!(
        &crate::i18n::plural_key("cli.clean-items", items.len()),
        count = items.len(),
        size = format_size(total_size(items))
    )
    .to_string()
}

pub(crate) fn cleanup_version(item: &CleanupItem) -> String {
    match &item.arch {
        Some(arch) if item.version.is_empty() => format!("({arch})"),
        Some(arch) => format!("{} ({arch})", item.version),
        None => item.version.clone(),
    }
}

type PreviewSplit<'a> = (Vec<(SourceType, &'a str)>, Vec<(SourceType, &'a PikeError)>);

pub(crate) fn split_clean_preview(
    preview: &[(SourceType, Result<Vec<String>, PikeError>)],
) -> PreviewSplit<'_> {
    let mut extras = Vec::new();
    let mut errors = Vec::new();
    for (st, result) in preview {
        match result {
            Ok(packages) => extras.extend(packages.iter().map(|p| (*st, p.as_str()))),
            Err(e) => errors.push((*st, e)),
        }
    }
    (extras, errors)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pike_core::package::CleanupKind;

    #[test]
    fn test_cleanup_version_with_arch() {
        let mut item = CleanupItem {
            source: SourceType::Flatpak,
            kind: CleanupKind::UnusedRuntime,
            name: "org.gnome.Platform".into(),
            version: "47".into(),
            size: None,
            arch: Some("x86_64".into()),
        };
        assert_eq!(cleanup_version(&item), "47 (x86_64)");
        item.arch = None;
        assert_eq!(cleanup_version(&item), "47");
        item.version.clear();
        item.arch = Some("x86_64".into());
        assert_eq!(cleanup_version(&item), "(x86_64)");
    }

    #[test]
    fn test_split_clean_preview_keeps_source_order() {
        let preview = vec![
            (SourceType::Dnf, Err(PikeError::Other("bad".into()))),
            (SourceType::Apt, Ok(vec!["a".into(), "b".into()])),
            (SourceType::Flatpak, Err(PikeError::Other("worse".into()))),
        ];
        let (extras, errors) = split_clean_preview(&preview);
        assert_eq!(extras, [(SourceType::Apt, "a"), (SourceType::Apt, "b")]);
        let sources: Vec<_> = errors.iter().map(|(st, _)| *st).collect();
        assert_eq!(sources, [SourceType::Dnf, SourceType::Flatpak]);
    }

    #[test]
    fn test_format_size() {
        assert_eq!(format_size(512), "512 B");
        assert_eq!(format_size(1023), "1023 B");
        assert_eq!(format_size(1024), "1.0 KiB");
        assert_eq!(format_size(1536), "1.5 KiB");
        assert_eq!(format_size(1_048_524), "1023.9 KiB");
        assert_eq!(format_size(1_048_530), "1.0 MiB");
        assert_eq!(format_size(113_989_117), "108.7 MiB");
        assert_eq!(format_size(1024 * 1024 * 1024 - 1), "1.0 GiB");
        assert_eq!(format_size(3 * 1024 * 1024 * 1024), "3.0 GiB");
    }
}
