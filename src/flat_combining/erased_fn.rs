pub struct ErasedFn {
    func: unsafe fn(*const u8, *mut u8),
    data: *const u8,
}

impl ErasedFn {
    pub fn new<T, F: FnOnce(&mut T)>(f: &F) -> Self {
        unsafe fn call_impl<T, F: FnOnce(&mut T)>(data: *const u8, arg: *mut u8) {
            let f = std::ptr::read(data as *const F);
            f(&mut *(arg as *mut T));
        }

        // Create a wrapper that has the exact signature we want
        let wrapper: unsafe fn(*const u8, *mut u8) = call_impl::<T, F>;

        ErasedFn {
            func: wrapper,
            data: f as *const F as *const u8,
        }
    }

    pub unsafe fn invoke<T>(self, arg: &mut T) {
        unsafe { (self.func)(self.data, arg as *mut T as *mut u8) };
    }
}
