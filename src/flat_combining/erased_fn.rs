pub struct ErasedFn<T> {
    func: unsafe fn(*const u8, &mut T),
    data: *const u8,
}

impl<T> ErasedFn<T> {
    pub fn new<F: FnOnce(&mut T)>(f: &F) -> Self {
        unsafe fn call_impl<T, F: FnOnce(&mut T)>(data: *const u8, arg: &mut T) {
            let f = std::ptr::read(data as *const F);
            f(arg);
        }

        // Create a wrapper that has the exact signature we want
        let wrapper: unsafe fn(*const u8, &mut T) = call_impl::<T, F>;

        ErasedFn {
            func: wrapper,
            data: f as *const F as *const u8,
        }
    }

    pub fn invoke(self, arg: &mut T) {
        unsafe { (self.func)(self.data, arg) };
    }
}
