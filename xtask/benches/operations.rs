use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use rust_crate_release::{
    Release,
    changelog::{Changelog, KeepAChangelog},
    execution::LocalExecutionProvider,
    hosting::ReleaseHost,
    markdown::MarkdownDocument,
    package::{Artifact, CargoPackage, PackageProject},
    registry::PackagePublisher,
    semantic::{CommitAnalyzer, ConventionalCommits, ReleaseRule, ReleaseType, SemanticCommit},
    testing::{DirectoryReleaseHost, GitProject, LocalRegistry},
};
use std::{fmt::Write, fs, hint::black_box, time::Duration};

fn changelog(entries: usize) -> String {
    let mut source = String::from("# Changelog\n\n## [Unreleased]\n\n### Added\n\n");
    for index in 0..entries {
        writeln!(
            source,
            "- Add feature {index} with Unicode support: \u{96ea}."
        )
        .unwrap();
    }
    source.push_str(
        "\n## [0.1.0] - 2026-01-01\n\n- Initial release.\n\n[Unreleased]: https://example.com/compare/v0.1.0...master\n[0.1.0]: https://example.com/releases/tag/v0.1.0\n",
    );
    source
}

fn documents(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("documents");
    let provider = KeepAChangelog::default();
    let version = "0.2.0".parse().unwrap();
    let next = "0.2.1".parse().unwrap();
    for entries in [10, 100, 1_000] {
        let source = changelog(entries);
        let released = provider
            .release(&source, &version, "2026/11/12", None)
            .unwrap();
        assert!(released.source.contains("## [0.2.0] - 2026-11-12"));
        assert!(
            provider
                .next(&released.source, &version, &next)
                .unwrap()
                .contains("## [Unreleased]")
        );
        group.throughput(Throughput::Bytes(source.len() as u64));
        group.bench_with_input(
            BenchmarkId::new("markdown_parse", entries),
            &source,
            |b, source| {
                b.iter(|| MarkdownDocument::parse(black_box(source.as_str())).unwrap());
            },
        );
        group.bench_with_input(
            BenchmarkId::new("changelog_release", entries),
            &source,
            |b, source| {
                b.iter(|| {
                    provider
                        .release(black_box(source), &version, "2026/11/12", None)
                        .unwrap()
                });
            },
        );
        group.throughput(Throughput::Bytes(released.source.len() as u64));
        group.bench_with_input(
            BenchmarkId::new("changelog_next", entries),
            &released.source,
            |b, source| {
                b.iter(|| provider.next(black_box(source), &version, &next).unwrap());
            },
        );
    }
    group.finish();
}

fn semantic(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("semantic");
    let default = ConventionalCommits::default();
    let rules = ConventionalCommits::new(vec![ReleaseRule {
        commit_type: Some("f[ei]*".into()),
        scope: Some("core".into()),
        subject: Some("improve *".into()),
        release: Some(ReleaseType::Minor),
        ..ReleaseRule::default()
    }]);
    for count in [10, 100, 1_000] {
        let commits: Vec<_> = (0..count)
            .map(|index| SemanticCommit {
                sha: format!("{index:040x}"),
                message: format!(
                    "{}(core): improve operation {index}",
                    if index % 2 == 0 { "feat" } else { "fix" }
                ),
            })
            .collect();
        for (name, analyzer) in [("default", &default), ("glob_rules", &rules)] {
            assert_eq!(
                analyzer.analyze(&commits).unwrap(),
                Some(ReleaseType::Minor)
            );
            group.throughput(Throughput::Elements(count as u64));
            group.bench_with_input(BenchmarkId::new(name, count), &commits, |b, commits| {
                b.iter(|| analyzer.analyze(black_box(commits)).unwrap());
            });
        }
    }
    group.finish();
}

fn archives(criterion: &mut Criterion) {
    let project = GitProject::cargo_library().unwrap();
    let mut seed = 42_u32;
    let payload: Vec<u8> = (0..256 * 1024)
        .map(|_| {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed as u8
        })
        .collect();
    fs::write(project.root().join("payload.bin"), payload).unwrap();
    let package = CargoPackage::new(project.root()).unwrap();
    let artifact = package
        .build(
            &project.temporary_directory().join("archive"),
            1_700_000_000,
        )
        .unwrap();
    let release = Release::new(
        artifact.name(),
        artifact.version().clone(),
        artifact.version().clone(),
        "a".repeat(40),
        "2026/11/12",
    )
    .unwrap();
    let host = DirectoryReleaseHost::new(project.temporary_directory().join("hosting")).unwrap();
    let registry = LocalRegistry::new(project.temporary_directory().join("registry")).unwrap();
    host.publish(&release, release.source_commit(), "- Benchmark.", &artifact)
        .unwrap();
    registry
        .publish(&artifact, &LocalExecutionProvider)
        .unwrap();
    let mut group = criterion.benchmark_group("archive");
    group.throughput(Throughput::Bytes(
        fs::metadata(artifact.path()).unwrap().len(),
    ));
    group.bench_function("open", |b| {
        b.iter(|| Artifact::open(black_box(artifact.path())).unwrap());
    });
    group.bench_function("verify", |b| {
        b.iter(|| black_box(&artifact).verify().unwrap());
    });
    group.bench_function("local_registry_reuse", |b| {
        b.iter(|| {
            registry
                .publish(black_box(&artifact), &LocalExecutionProvider)
                .unwrap()
        });
    });
    group.bench_function("local_host_reconcile", |b| {
        b.iter(|| {
            host.publish(
                &release,
                release.source_commit(),
                "- Benchmark.",
                black_box(&artifact),
            )
            .unwrap()
        });
    });
    group.finish();
}

criterion_group! {
    name = operations;
    config = Criterion::default()
        .sample_size(20)
        .warm_up_time(Duration::from_secs(1))
        .measurement_time(Duration::from_secs(2));
    targets = documents, semantic, archives
}
criterion_main!(operations);
