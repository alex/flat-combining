pub trait Mutator<T> {
    fn new(value: T) -> Self;

    fn mutate(&self, f: impl FnOnce(&mut T) + Send);
}

mod mutex;

pub use crate::mutex::Mutex;
