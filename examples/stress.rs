use flat_combining::{FlatCombining, Mutator};

fn main() {
    const N_ITER: usize = 1024;
    const N_THREADS: usize = 32;
    const N_OPS: usize = 128;

    for i in 0..N_ITER {
        println!("{i}");
        let m = FlatCombining::new(0);
        let b = std::sync::Barrier::new(N_THREADS);

        std::thread::scope(|s| {
            for _ in 0..N_THREADS {
                s.spawn(|| {
                    b.wait();

                    for _ in 0..N_OPS {
                        m.mutate(|v| *v += 1);
                    }
                });
            }
        });

        m.mutate(|v| {
            assert_eq!(*v, N_THREADS * N_OPS);
        });
    }
}
