use std::sync;

pub struct Mutex<T> {
    data: sync::Mutex<T>,
}

impl<T> Mutex<T> {
    pub fn into_inner(self) -> T {
        self.data.into_inner().expect("Mutex was poisoned")
    }
}

impl<T> crate::Mutator<T> for Mutex<T> {
    fn new(value: T) -> Self {
        Self {
            data: sync::Mutex::new(value),
        }
    }

    fn mutate(&self, f: impl FnOnce(&mut T) + Send) {
        let mut guard = self.data.lock().expect("Mutex was poisoned");
        f(&mut *guard)
    }
}
