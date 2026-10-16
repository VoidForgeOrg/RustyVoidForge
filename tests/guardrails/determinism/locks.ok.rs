// Control for locks.rs: single-threaded shared state is allowed.
pub struct Shared {
    pub counter: std::rc::Rc<std::cell::RefCell<u64>>,
    pub flag: std::cell::Cell<bool>,
}
