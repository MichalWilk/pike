use std::path::{Path, PathBuf};

use crate::error::PikeError;
use crate::package::{CleanupItem, CleanupKind, CleanupScan, SourceType};
use crate::source::{PackageSource, Result, run_captured_c_full, run_privileged};
use crate::util::{compare_versions, truncate_str};

pub fn total_size(items: &[CleanupItem]) -> u64 {
    items.iter().filter_map(|i| i.size).sum()
}

pub async fn scan_cleanup(
    sources: &[&dyn PackageSource],
    kinds: &[CleanupKind],
    keep_kernels: usize,
) -> CleanupScan {
    let futures: Vec<_> = sources
        .iter()
        .map(|s| s.list_cleanup(kinds, keep_kernels))
        .collect();
    let results = futures::future::join_all(futures).await;
    let mut scan = CleanupScan::default();
    for (source, result) in sources.iter().zip(results) {
        match result {
            Ok(items) => scan.items.extend(items),
            Err(e) => {
                tracing::warn!("list_cleanup {} failed: {}", source.name(), e);
                scan.failed.push((source.source_type(), e.to_string()));
            }
        }
    }
    scan.items.sort_by(|a, b| {
        a.kind
            .cmp(&b.kind)
            .then_with(|| a.source.cmp(&b.source))
            .then_with(|| {
                if a.kind == CleanupKind::OldKernel {
                    compare_versions(&b.version, &a.version)
                } else {
                    std::cmp::Ordering::Equal
                }
            })
            .then_with(|| a.name.cmp(&b.name))
    });
    scan
}

pub async fn preview_cleanup(
    sources: &[&dyn PackageSource],
    items: &[CleanupItem],
) -> Vec<(SourceType, Result<Vec<String>>)> {
    let mut results = Vec::new();
    for (st, source, subset) in group_by_source(sources, items) {
        let result = match source {
            Ok(s) => s.preview_clean(&subset).await,
            Err(e) => Err(e),
        };
        results.push((st, result));
    }
    results
}

pub async fn clean_items(
    sources: &[&dyn PackageSource],
    items: &[CleanupItem],
) -> Vec<(SourceType, Result<()>)> {
    let mut results = Vec::new();
    for (st, source, subset) in group_by_source(sources, items) {
        let result = match source {
            Ok(s) => s.clean(&subset).await,
            Err(e) => Err(e),
        };
        results.push((st, result));
    }
    results
}

fn group_by_source<'a>(
    sources: &[&'a dyn PackageSource],
    items: &[CleanupItem],
) -> Vec<(SourceType, Result<&'a dyn PackageSource>, Vec<CleanupItem>)> {
    SourceType::ALL
        .iter()
        .filter_map(|&st| {
            let subset: Vec<CleanupItem> =
                items.iter().filter(|i| i.source == st).cloned().collect();
            if subset.is_empty() {
                return None;
            }
            let source = sources
                .iter()
                .find(|s| s.source_type() == st)
                .copied()
                .ok_or_else(|| PikeError::Other(format!("{st} source is not active")));
            Some((st, source, subset))
        })
        .collect()
}

pub(crate) async fn remove_and_clean_cache(
    items: &[CleanupItem],
    packages: &[String],
    remove_cmd: &[&str],
    cache_cmd: &[&str],
) -> Result<()> {
    if !packages.is_empty() {
        let mut args = remove_cmd.to_vec();
        args.extend(packages.iter().map(String::as_str));
        run_privileged(&args).await?;
    }
    if of_kind(items, CleanupKind::Cache).next().is_some() {
        run_privileged(cache_cmd).await?;
    }
    Ok(())
}

pub(crate) async fn preview_removal(
    source: SourceType,
    packages: &[String],
    cmd: &str,
    args: &[&str],
    allowed_codes: &[i32],
    parse: fn(&str) -> Vec<String>,
) -> Result<Vec<String>> {
    if packages.is_empty() {
        return Ok(Vec::new());
    }
    let mut args = args.to_vec();
    args.extend(packages.iter().map(String::as_str));
    let output = run_captured_c_full(cmd, &args, allowed_codes).await?;
    extra_removals(parse(&output.stdout), packages, source)
        .map_err(|e| with_stderr(e, &output.stderr))
}

pub(crate) fn with_stderr(err: PikeError, stderr: &str) -> PikeError {
    match err {
        PikeError::Parse {
            source_name,
            detail,
        } if !stderr.trim().is_empty() => {
            let flat = stderr
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .collect::<Vec<_>>()
                .join("; ");
            PikeError::Parse {
                source_name,
                detail: format!("{detail}: {}", truncate_str(&flat, 300)),
            }
        }
        other => other,
    }
}

/// A requested package missing from `removed` means the simulation failed or found nothing.
pub(crate) fn extra_removals(
    removed: Vec<String>,
    requested: &[String],
    source: SourceType,
) -> Result<Vec<String>> {
    if let Some(missing) = requested.iter().find(|r| !removed.contains(r)) {
        return Err(PikeError::Parse {
            source_name: source.display_name().to_string(),
            detail: format!("removal preview does not list requested package {missing}"),
        });
    }
    Ok(removed
        .into_iter()
        .filter(|r| !requested.contains(r))
        .collect())
}

/// Keeps the `keep` newest kernels and the running one; offers nothing when the running kernel is unknown.
pub(crate) fn old_kernel_items(
    mut versions: Vec<(String, Option<u64>)>,
    running: Option<&str>,
    keep: usize,
    source: SourceType,
    name: impl Fn(&str) -> String,
) -> Vec<CleanupItem> {
    let Some(running) = running else {
        tracing::warn!(
            "{}: could not determine running kernel, skipping old kernel detection",
            source.display_name()
        );
        return Vec::new();
    };
    versions.sort_by(|a, b| compare_versions(&b.0, &a.0));
    versions
        .into_iter()
        .skip(keep.max(1))
        .filter(|(v, _)| v != running)
        .map(|(version, size)| CleanupItem {
            source,
            kind: CleanupKind::OldKernel,
            name: name(&version),
            version,
            size,
            arch: None,
        })
        .collect()
}

/// Kernel packages of the running release are dropped, all of them when it is unknown.
pub(crate) fn removal_list(
    mut packages: Vec<String>,
    kernel_pkgs: Vec<String>,
    running: Option<&str>,
) -> Vec<String> {
    packages.extend(kernel_pkgs.into_iter().filter(|p| {
        let keep = running.is_some_and(|r| !p.ends_with(&format!("-{r}")));
        if !keep {
            tracing::warn!(
                "not removing kernel package {p}: matches running kernel or running kernel unknown"
            );
        }
        keep
    }));
    packages.sort();
    packages.dedup();
    packages
}

pub(crate) fn of_kind(
    items: &[CleanupItem],
    kind: CleanupKind,
) -> impl Iterator<Item = &CleanupItem> {
    items.iter().filter(move |i| i.kind == kind)
}

pub(crate) async fn cache_item(source: SourceType, dir: &str) -> Option<CleanupItem> {
    let path = PathBuf::from(dir);
    let size = tokio::task::spawn_blocking(move || match std::fs::read_dir(&path) {
        Ok(entries) => entries_size(entries),
        Err(e) => {
            tracing::debug!("cannot read cache dir {}: {e}", path.display());
            0
        }
    })
    .await
    .unwrap_or(0);
    (size > 0).then(|| CleanupItem {
        source,
        kind: CleanupKind::Cache,
        name: dir.to_string(),
        version: String::new(),
        size: Some(size),
        arch: None,
    })
}

/// `uname -r` without a Fedora variant tag (`+debug`, `+16k`); Debian releases such as
/// `6.12.38+deb13-amd64` keep their `+` part, which belongs to the version.
pub(crate) fn running_kernel() -> Option<String> {
    std::fs::read_to_string("/proc/sys/kernel/osrelease")
        .ok()
        .map(|s| kernel_release(s.trim()))
}

fn kernel_release(osrelease: &str) -> String {
    match osrelease.rsplit_once('+') {
        Some((base, tag)) if !tag.is_empty() && !tag.contains('-') => base.to_string(),
        _ => osrelease.to_string(),
    }
}

fn dir_size(path: &Path) -> u64 {
    std::fs::read_dir(path).map_or(0, entries_size)
}

fn entries_size(entries: std::fs::ReadDir) -> u64 {
    entries
        .flatten()
        .map(|entry| match entry.file_type() {
            Ok(ft) if ft.is_dir() => dir_size(&entry.path()),
            Ok(ft) if ft.is_file() => entry.metadata().map(|m| m.len()).unwrap_or(0),
            _ => 0,
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::{Package, PackageUpdate};
    use async_trait::async_trait;

    fn v(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    type KernelCase<'a> = (&'a [&'a str], Option<&'a str>, usize, &'a [&'a str]);

    #[test]
    fn test_old_kernel_items() {
        let fedora = [
            "7.2.5-200.fc44.x86_64",
            "7.2.6-200.fc44.x86_64",
            "7.2.7-200.fc44.x86_64",
        ];
        let ubuntu = [
            "6.8.0-90-generic",
            "6.8.0-100-generic",
            "6.8.0-45-generic",
            "6.8.0-110-generic",
            "6.8.0-60-generic",
        ];
        let cases: [KernelCase; 7] = [
            (
                &fedora,
                Some("7.2.7-200.fc44.x86_64"),
                2,
                &["7.2.5-200.fc44.x86_64"],
            ),
            (
                &fedora,
                Some("7.2.5-200.fc44.x86_64"),
                1,
                &["7.2.6-200.fc44.x86_64"],
            ),
            (&ubuntu[..2], Some("6.8.0-100-generic"), 5, &[]),
            (
                &ubuntu[..3],
                Some("6.8.0-100-generic"),
                0,
                &["6.8.0-90-generic", "6.8.0-45-generic"],
            ),
            (
                &ubuntu,
                Some("6.8.0-110-generic"),
                2,
                &["6.8.0-90-generic", "6.8.0-60-generic", "6.8.0-45-generic"],
            ),
            (&ubuntu, None, 1, &[]),
            (&[], Some("6.8.0-110-generic"), 1, &[]),
        ];
        for (installed, running, keep, expected) in cases {
            let versions = installed.iter().map(|s| (s.to_string(), None)).collect();
            let items = old_kernel_items(versions, running, keep, SourceType::Apt, |v| {
                format!("linux-image-{v}")
            });
            let got: Vec<&str> = items.iter().map(|i| i.version.as_str()).collect();
            assert_eq!(
                got, expected,
                "{installed:?} running {running:?} keep {keep}"
            );
            assert!(items.iter().all(|i| i.kind == CleanupKind::OldKernel
                && i.name == format!("linux-image-{}", i.version)));
        }
    }

    #[test]
    fn test_old_kernel_items_sizes() {
        let versions = vec![
            ("7.2.5-200.fc44.x86_64".to_string(), Some(10)),
            ("7.2.7-200.fc44.x86_64".to_string(), Some(30)),
            ("7.2.6-200.fc44.x86_64".to_string(), None),
        ];
        let items = old_kernel_items(
            versions,
            Some("7.2.7-200.fc44.x86_64"),
            1,
            SourceType::Dnf,
            |_| "kernel".to_string(),
        );
        let sizes: Vec<Option<u64>> = items.iter().map(|i| i.size).collect();
        assert_eq!(sizes, vec![None, Some(10)]);
    }

    #[test]
    fn test_kernel_release() {
        let cases = [
            ("7.2.5-200.fc44.x86_64+debug", "7.2.5-200.fc44.x86_64"),
            ("7.2.5-200.fc44.aarch64+16k", "7.2.5-200.fc44.aarch64"),
            ("7.2.5-200.fc44.x86_64", "7.2.5-200.fc44.x86_64"),
            ("6.12.38+deb13-amd64", "6.12.38+deb13-amd64"),
            ("6.12.9+bpo-amd64", "6.12.9+bpo-amd64"),
            ("6.6.51+rpt-rpi-v8", "6.6.51+rpt-rpi-v8"),
            ("6.8.0-45-generic", "6.8.0-45-generic"),
        ];
        for (input, expected) in cases {
            assert_eq!(kernel_release(input), expected, "{input}");
        }
    }

    #[test]
    fn test_removal_list() {
        let orphans = v(&["libfoo", "libbar", "libfoo"]);
        let kernels = v(&[
            "kernel-core-7.2.6-200.fc44.x86_64",
            "kernel-core-7.2.5-200.fc44.x86_64",
            "kernel-core-17.2.5-200.fc44.x86_64",
            "libbar",
        ]);
        assert_eq!(
            removal_list(
                orphans.clone(),
                kernels.clone(),
                Some("7.2.6-200.fc44.x86_64")
            ),
            v(&[
                "kernel-core-17.2.5-200.fc44.x86_64",
                "kernel-core-7.2.5-200.fc44.x86_64",
                "libbar",
                "libfoo"
            ])
        );
        assert_eq!(
            removal_list(
                orphans.clone(),
                kernels.clone(),
                Some("7.2.5-200.fc44.x86_64")
            ),
            v(&[
                "kernel-core-17.2.5-200.fc44.x86_64",
                "kernel-core-7.2.6-200.fc44.x86_64",
                "libbar",
                "libfoo"
            ])
        );
        assert_eq!(
            removal_list(orphans, kernels, None),
            v(&["libbar", "libfoo"])
        );
    }

    #[tokio::test]
    async fn test_cache_item() {
        assert!(
            cache_item(SourceType::Apt, "/nonexistent/pike-test")
                .await
                .is_none()
        );

        let root = std::env::temp_dir().join(format!("pike-cache-item-{}", std::process::id()));
        let nested = root.join("a").join("b");
        std::fs::create_dir_all(&nested).unwrap();
        let dir = root.to_str().unwrap().to_string();
        let empty = cache_item(SourceType::Apt, &dir).await;
        std::fs::write(root.join("top"), [0u8; 100]).unwrap();
        std::fs::write(nested.join("deep"), [0u8; 23]).unwrap();
        let item = cache_item(SourceType::Apt, &dir).await;
        std::fs::remove_dir_all(&root).unwrap();

        assert!(empty.is_none());
        let item = item.unwrap();
        assert_eq!(item.kind, CleanupKind::Cache);
        assert_eq!(item.name, dir);
        assert_eq!(item.size, Some(123));
    }

    #[test]
    fn test_extra_removals() {
        let requested = v(&["a"]);
        assert_eq!(
            extra_removals(v(&["b", "a", "c"]), &requested, SourceType::Apt).unwrap(),
            v(&["b", "c"])
        );
        assert!(matches!(
            extra_removals(Vec::new(), &requested, SourceType::Apt),
            Err(PikeError::Parse { .. })
        ));
        assert!(
            extra_removals(Vec::new(), &[], SourceType::Apt)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn test_with_stderr() {
        let parse = || PikeError::Parse {
            source_name: "dnf".into(),
            detail: "missing libfoo".into(),
        };
        match with_stderr(parse(), "No match for argument: libfoo\n") {
            PikeError::Parse { detail, .. } => {
                assert_eq!(detail, "missing libfoo: No match for argument: libfoo")
            }
            e => panic!("unexpected {e:?}"),
        }
        match with_stderr(parse(), " \n") {
            PikeError::Parse { detail, .. } => assert_eq!(detail, "missing libfoo"),
            e => panic!("unexpected {e:?}"),
        }
        assert!(matches!(
            with_stderr(PikeError::Other("x".into()), "err"),
            PikeError::Other(_)
        ));
    }

    struct StubSource {
        st: SourceType,
        items: Option<Vec<CleanupItem>>,
    }

    #[async_trait]
    impl PackageSource for StubSource {
        fn name(&self) -> &str {
            "stub"
        }
        fn source_type(&self) -> SourceType {
            self.st
        }
        async fn search(&self, _query: &str) -> Result<Vec<Package>> {
            Ok(Vec::new())
        }
        async fn install(&self, _package: &str) -> Result<()> {
            Ok(())
        }
        async fn remove(&self, _package: &str, _purge: bool) -> Result<()> {
            Ok(())
        }
        async fn check_updates(&self) -> Result<Vec<PackageUpdate>> {
            Ok(Vec::new())
        }
        async fn update(&self, _package: &str) -> Result<()> {
            Ok(())
        }
        async fn update_all(&self) -> Result<()> {
            Ok(())
        }
        async fn list_installed(&self) -> Result<Vec<Package>> {
            Ok(Vec::new())
        }
        async fn list_cleanup(
            &self,
            kinds: &[CleanupKind],
            _keep_kernels: usize,
        ) -> Result<Vec<CleanupItem>> {
            let items = self
                .items
                .clone()
                .ok_or_else(|| PikeError::Other("stub failure".into()))?;
            Ok(items
                .into_iter()
                .filter(|i| kinds.contains(&i.kind))
                .collect())
        }
        async fn clean(&self, items: &[CleanupItem]) -> Result<()> {
            if items.iter().all(|i| i.source == self.st) {
                Ok(())
            } else {
                Err(PikeError::Other("foreign item".into()))
            }
        }
        async fn preview_clean(&self, items: &[CleanupItem]) -> Result<Vec<String>> {
            Ok(items.iter().map(|i| format!("{}-dep", i.name)).collect())
        }
    }

    fn kernel(version: &str) -> CleanupItem {
        CleanupItem {
            source: SourceType::Apt,
            kind: CleanupKind::OldKernel,
            name: format!("linux-image-{version}"),
            version: version.to_string(),
            size: None,
            arch: None,
        }
    }

    fn stub(st: SourceType, items: Option<Vec<CleanupItem>>) -> StubSource {
        StubSource { st, items }
    }

    #[tokio::test]
    async fn test_scan_cleanup_orders_kernels_newest_first() {
        let apt = stub(
            SourceType::Apt,
            Some(vec![
                kernel("6.8.0-45-generic"),
                kernel("6.8.0-100-generic"),
                kernel("6.8.0-90-generic"),
            ]),
        );
        let scan = scan_cleanup(&[&apt], &CleanupKind::ALL, 2).await;
        let versions: Vec<&str> = scan.items.iter().map(|i| i.version.as_str()).collect();
        assert_eq!(
            versions,
            vec!["6.8.0-100-generic", "6.8.0-90-generic", "6.8.0-45-generic"]
        );
    }

    #[tokio::test]
    async fn test_scan_cleanup_failed_source_and_kinds() {
        let ok = stub(SourceType::Apt, Some(vec![kernel("6.8.0-45-generic")]));
        let failing = stub(SourceType::Flatpak, None);
        let sources: [&dyn PackageSource; 2] = [&ok, &failing];
        let scan = scan_cleanup(&sources, &[CleanupKind::OldKernel], 1).await;
        assert_eq!(scan.items.len(), 1);
        assert_eq!(scan.failed.len(), 1);
        assert_eq!(scan.failed[0].0, SourceType::Flatpak);

        let scan = scan_cleanup(&sources, &[CleanupKind::Cache], 1).await;
        assert!(scan.items.is_empty());
    }

    fn mixed_items() -> Vec<CleanupItem> {
        let mut dnf_item = kernel("7.2.5-200.fc44.x86_64");
        dnf_item.source = SourceType::Dnf;
        let mut flatpak_item = kernel("49");
        flatpak_item.source = SourceType::Flatpak;
        vec![
            kernel("6.8.0-45-generic"),
            flatpak_item,
            dnf_item,
            kernel("6.8.0-90-generic"),
        ]
    }

    #[tokio::test]
    async fn test_preview_cleanup_groups_by_source() {
        let apt = stub(SourceType::Apt, None);
        let flatpak = stub(SourceType::Flatpak, None);
        let results = preview_cleanup(&[&apt, &flatpak], &mixed_items()).await;
        let order: Vec<SourceType> = results.iter().map(|(st, _)| *st).collect();
        assert_eq!(
            order,
            vec![SourceType::Dnf, SourceType::Flatpak, SourceType::Apt]
        );
        assert!(matches!(results[0].1, Err(PikeError::Other(_))));
        assert_eq!(
            results[2].1.as_ref().unwrap(),
            &v(&[
                "linux-image-6.8.0-45-generic-dep",
                "linux-image-6.8.0-90-generic-dep"
            ])
        );
    }

    #[tokio::test]
    async fn test_clean_items_orders_by_source() {
        let apt = stub(SourceType::Apt, None);
        let flatpak = stub(SourceType::Flatpak, None);
        let results = clean_items(&[&apt, &flatpak], &mixed_items()).await;
        let order: Vec<SourceType> = results.iter().map(|(st, _)| *st).collect();
        assert_eq!(
            order,
            vec![SourceType::Dnf, SourceType::Flatpak, SourceType::Apt]
        );
        assert!(matches!(results[0].1, Err(PikeError::Other(_))));
        assert!(results[1].1.is_ok());
        assert!(results[2].1.is_ok());
        assert!(clean_items(&[&apt], &[]).await.is_empty());
    }
}
