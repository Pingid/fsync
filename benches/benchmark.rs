use criterion::{criterion_group, criterion_main, BatchSize, BenchmarkId, Criterion};
use fsync::{temp_fs, Synchronize};

fn benchmark(c: &mut Criterion) {
    let mut group = c.benchmark_group("syncing directory");

    group.warm_up_time(std::time::Duration::new(5, 0)); // Set the warm up time to 5 seconds
    group.measurement_time(std::time::Duration::new(10, 0)); // Set the measurement time to 10 seconds
    group.sample_size(10); // Set the sample size to 50

    group.bench_function(BenchmarkId::new("benchmark_sync", "sync"), |b| {
        b.iter_batched_ref(
            || {
                let temp = temp_fs!(
                    one / f1: 10 * 1024 * 1024,
                    one / f2: 10 * 1024 * 1024,
                    one / two / f1: 10 * 1024 * 1024,
                    one / two / f2: 10 * 1024 * 1024,
                    one / two / three / f1: 100 * 1024 * 1024,
                    one / two / three / four / f1: 1000 * 1024 * 1024,
                );
                (temp.path().join("input"), temp.path().join("output"))
            },
            |(input, output)| Synchronize::new(input.clone(), output.clone()).sync(),
            BatchSize::SmallInput,
        )
    });

    group.finish();
}

criterion_group!(benches, benchmark);
criterion_main!(benches);
