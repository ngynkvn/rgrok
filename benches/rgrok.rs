use std::str::FromStr;

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};

use rgrok::{rgrok_dir, rgrok_dir_parallel, Args, Output};
use syntect::{highlighting::ThemeSet, parsing::SyntaxSet};

fn criterion_benchmark(c: &mut Criterion) {
    let mut group = c.benchmark_group("rgrok");
    let ps = SyntaxSet::load_defaults_newlines();
    let ts = ThemeSet::load_defaults();
    group.sample_size(10);
    group.bench_with_input(
        BenchmarkId::new("single-thread", "Self"),
        &Args {
            path: ".".into(),
            regex: regex::Regex::from_str("fn").unwrap(),
            parallel: false,
            output: Output::Null,
        },
        |b, i| b.iter(|| rgrok_dir(i.clone(), &ps, &ts)),
    );
    group.bench_with_input(
        BenchmarkId::new("parallel", "Self"),
        &Args {
            path: ".".into(),
            regex: regex::Regex::from_str("fn").unwrap(),
            parallel: true,
            output: Output::Null,
        },
        |b, i| b.iter(|| rgrok_dir_parallel(i.clone(), &ps, &ts)),
    );
}

criterion_group!(
    name = benches;
    config = Criterion::default();
    targets = criterion_benchmark
);
criterion_main!(benches);
