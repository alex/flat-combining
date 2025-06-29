use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use magic_structure::{Mutator, Mutex};
use std::sync::Barrier;
use std::thread;
use std::time::Duration;

fn bench_mutator_implementation<M: Mutator<u64> + Sync>(
    group: &mut criterion::BenchmarkGroup<criterion::measurement::WallTime>,
    impl_name: &str,
) where
    M: 'static,
{
    group.sample_size(10);

    let work_durations = [
        Duration::from_nanos(0),
        Duration::from_nanos(100),
        Duration::from_micros(1),
        Duration::from_micros(10),
    ];

    let thread_counts = [2, 4, 8, 16, 32];
    let operations_per_thread = 100;

    for &work_duration in &work_durations {
        for &thread_count in &thread_counts {
            let param_name = format!(
                "{}_{}μs_{}threads",
                impl_name,
                work_duration.as_micros(),
                thread_count
            );
            group.throughput(Throughput::Elements(
                (thread_count * operations_per_thread) as u64,
            ));

            group.bench_with_input(
                BenchmarkId::new("workload", param_name),
                &(work_duration, thread_count),
                |b, &(work_duration, thread_count)| {
                    b.iter(|| {
                        let wrapper = M::new(0u64);
                        let barrier = Barrier::new(thread_count);

                        thread::scope(|s| {
                            for _ in 0..thread_count {
                                s.spawn(|| {
                                    // Wait for all threads to be ready
                                    barrier.wait();

                                    for _ in 0..operations_per_thread {
                                        wrapper.mutate(|counter| {
                                            if work_duration > Duration::ZERO {
                                                thread::sleep(work_duration);
                                            }
                                            *counter += 1;
                                        });
                                    }
                                });
                            }
                        });
                    });
                },
            );
        }
    }
}

fn bench_workload_matrix(c: &mut Criterion) {
    let mut group = c.benchmark_group("workload_matrix");
    bench_mutator_implementation::<Mutex<u64>>(&mut group, "mutex");
}

criterion_group!(benches, bench_workload_matrix);
criterion_main!(benches);
