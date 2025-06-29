pub trait Mutator<T> {
    fn new(value: T) -> Self;

    fn mutate<F: FnOnce(&mut T) + Send>(&self, f: F);
}

mod flat_combining;
mod mutex;

pub use crate::flat_combining::FlatCombining;
pub use crate::mutex::Mutex;
