// Determinism fixture: locks and other concurrency primitives. The core is single-threaded.
// expect: clippy::disallowed_types std::sync::Mutex
// expect: clippy::disallowed_types std::sync::RwLock
// expect: clippy::disallowed_types std::sync::Condvar
// expect: clippy::disallowed_types std::sync::Barrier
// expect: clippy::disallowed_types std::sync::Once
// expect: clippy::disallowed_types std::sync::OnceLock
// expect: clippy::disallowed_types std::sync::LazyLock
// expect: clippy::disallowed_types std::sync::mpsc::Sender
// expect: clippy::disallowed_types std::sync::mpsc::SyncSender
// expect: clippy::disallowed_types std::sync::mpsc::Receiver
// expect: clippy::disallowed_methods std::sync::mpsc::channel
// expect: clippy::disallowed_methods std::sync::mpsc::sync_channel
// expect: clippy::disallowed_types std::sync::atomic::AtomicBool
// expect: clippy::disallowed_types std::sync::atomic::AtomicI8
// expect: clippy::disallowed_types std::sync::atomic::AtomicI16
// expect: clippy::disallowed_types std::sync::atomic::AtomicI32
// expect: clippy::disallowed_types std::sync::atomic::AtomicI64
// expect: clippy::disallowed_types std::sync::atomic::AtomicIsize
// expect: clippy::disallowed_types std::sync::atomic::AtomicU8
// expect: clippy::disallowed_types std::sync::atomic::AtomicU16
// expect: clippy::disallowed_types std::sync::atomic::AtomicU32
// expect: clippy::disallowed_types std::sync::atomic::AtomicU64
// expect: clippy::disallowed_types std::sync::atomic::AtomicUsize
// expect: clippy::disallowed_types std::sync::atomic::AtomicPtr

pub struct Locks {
    pub mutex: std::sync::Mutex<u64>,
    pub rw_lock: std::sync::RwLock<u64>,
    pub condvar: std::sync::Condvar,
    pub barrier: std::sync::Barrier,
    pub once: std::sync::Once,
    pub once_lock: std::sync::OnceLock<u64>,
    pub lazy_lock: std::sync::LazyLock<u64>,
}

pub struct Atomics {
    pub bool: std::sync::atomic::AtomicBool,
    pub i8: std::sync::atomic::AtomicI8,
    pub i16: std::sync::atomic::AtomicI16,
    pub i32: std::sync::atomic::AtomicI32,
    pub i64: std::sync::atomic::AtomicI64,
    pub isize: std::sync::atomic::AtomicIsize,
    pub u8: std::sync::atomic::AtomicU8,
    pub u16: std::sync::atomic::AtomicU16,
    pub u32: std::sync::atomic::AtomicU32,
    pub u64: std::sync::atomic::AtomicU64,
    pub usize: std::sync::atomic::AtomicUsize,
    pub ptr: std::sync::atomic::AtomicPtr<u8>,
}

pub fn channel() -> (std::sync::mpsc::Sender<u64>, std::sync::mpsc::Receiver<u64>) {
    std::sync::mpsc::channel()
}

pub fn sync_channel() -> (std::sync::mpsc::SyncSender<u64>, std::sync::mpsc::Receiver<u64>) {
    std::sync::mpsc::sync_channel(1)
}
