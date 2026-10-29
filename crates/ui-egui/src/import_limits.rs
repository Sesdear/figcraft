//! Admission and memory accounting for dropped files, including results waiting on other reads.
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};

/// Ceiling for byte-backed inputs; native path imports keep the engine's existing file policy.
/// The aggregate pipeline reservation can reject smaller byte-backed files too.
pub(crate) const FILE_LIMIT: usize = 512 << 20;
// One GiB of charged input storage, including overlapping encoding/decoding copies. With the
// twelve-fold charge below, one byte-backed input must be under about 85 MiB even when the
// budget is empty. Native paths only use admission slots: they are not buffered in PendingOpen.
const MEMORY_LIMIT: usize = 1 << 30;
/// Bound handles, queued futures and completed imports, across windows and drop gestures.
pub(crate) const PENDING_LIMIT: usize = 64;
const READERS: usize = 2;
const OVERHEAD: usize = 64 << 10;

/// Process-wide limits: pending work includes queued, reading and completed imports.
pub(crate) struct Limits {
    memory: AtomicUsize,
    pending: AtomicUsize,
    readers: AtomicUsize,
    capacity: usize,
}

impl Limits {
    /// Creates an independent byte budget with the same queue and reader caps as the global one.
    pub(crate) fn new(capacity: usize) -> Self {
        Self { memory: AtomicUsize::new(0), pending: AtomicUsize::new(0), readers: AtomicUsize::new(0), capacity }
    }

    /// Shares admission across windows, gestures and completed imports waiting in the UI.
    pub(crate) fn shared() -> Arc<Self> {
        static LIMITS: OnceLock<Arc<Limits>> = OnceLock::new();
        LIMITS.get_or_init(|| Arc::new(Self::new(MEMORY_LIMIT))).clone()
    }

    #[cfg(test)]
    pub(crate) fn usage(&self) -> (usize, usize, usize) {
        (self.memory.load(Ordering::Acquire), self.pending.load(Ordering::Acquire), self.readers.load(Ordering::Acquire))
    }

    /// Reject excess work before retaining a file handle or creating a future/thread.
    pub(crate) fn admit(self: &Arc<Self>) -> Option<ImportLease> {
        self.pending.fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| (n < PENDING_LIMIT).then_some(n + 1)).ok()?;
        Some(ImportLease(Arc::new(Reservation { limits: self.clone(), bytes: AtomicUsize::new(0) })))
    }

    /// At most two native read threads exist at once. Queued futures retry on the next UI frame.
    pub(crate) fn reader(self: &Arc<Self>) -> Option<Reader> {
        self.readers.fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| (n < READERS).then_some(n + 1)).ok()?;
        Some(Reader(self.clone()))
    }
}

/// Shared by the reader, its future, PendingOpen and the repeat-command owner. The last owner
/// releases both admission and bytes, including cancellation, failed send, spawn failure and unwind.
#[derive(Clone)]
pub(crate) struct ImportLease(Arc<Reservation>);

impl ImportLease {
    /// Reserve before allocating. Twelve times the input covers raw/decoded buffers and overlapping
    /// base64 strings in run_user, engine parameter extraction and image.place (4/3 expansion each).
    /// The charge stays live while encoded PendingOpen data or edit.repeat parameters retain it.
    /// Parser output, image decompression, document/undo storage and backend-owned data are separate.
    pub(crate) fn reserve_file(&self, size: usize) -> bool {
        size <= FILE_LIMIT && size.checked_mul(12).and_then(|n| n.checked_add(OVERHEAD)).is_some_and(|n| self.resize(n))
    }

    /// Unknown-size native virtual handles reserve the maximum raw read before calling the backend,
    /// then replace it with the full pipeline charge once their actual length is known.
    pub(crate) fn reserve_virtual_read(&self) -> bool {
        self.resize(FILE_LIMIT)
    }

    /// Changes the byte charge atomically against the shared budget; failure keeps the old charge.
    /// Only the reader may resize a lease; other owners only retain it until their data is freed.
    fn resize(&self, bytes: usize) -> bool {
        let previous = self.0.bytes.load(Ordering::Acquire);
        if bytes > previous {
            let extra = bytes - previous;
            if self
                .0
                .limits
                .memory
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |used| used.checked_add(extra).filter(|total| *total <= self.0.limits.capacity))
                .is_err()
            {
                return false;
            }
        } else {
            self.0.limits.memory.fetch_sub(previous - bytes, Ordering::AcqRel);
        }
        // Only the owning read calls resize; clones only extend the reservation's lifetime.
        self.0.bytes.store(bytes, Ordering::Release);
        true
    }
}

struct Reservation {
    limits: Arc<Limits>,
    bytes: AtomicUsize,
}

impl Drop for Reservation {
    /// Refunds admission and bytes exactly once when the final lease owner leaves scope.
    fn drop(&mut self) {
        self.limits.memory.fetch_sub(self.bytes.load(Ordering::Acquire), Ordering::AcqRel);
        self.limits.pending.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Releases a read slot on completion, spawn failure, cancellation or unwinding.
pub(crate) struct Reader(Arc<Limits>);
impl Drop for Reader {
    /// Makes a read slot available even when its thread fails to spawn or unwinds.
    fn drop(&mut self) {
        self.0.readers.fetch_sub(1, Ordering::AcqRel);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aggregate_budget_and_retained_results() {
        let limits = Arc::new(Limits::new(OVERHEAD * 2 + 120));
        let first = limits.admit().unwrap();
        assert!(first.reserve_file(5));
        let retained = first.clone();
        drop(first);
        let second = limits.admit().unwrap();
        assert!(second.reserve_file(5));
        let third = limits.admit().unwrap();
        assert!(!third.reserve_file(1));
        drop(retained);
        assert!(third.reserve_file(5));
        drop((second, third));
        assert_eq!(limits.memory.load(Ordering::Acquire), 0);
        assert_eq!(limits.pending.load(Ordering::Acquire), 0);
    }

    #[test]
    fn admission_and_readers_are_bounded_and_reusable() {
        let limits = Arc::new(Limits::new(MEMORY_LIMIT));
        for _ in 0..10 {
            let pending: Vec<_> = (0..PENDING_LIMIT).map(|_| limits.admit().unwrap()).collect();
            assert!((0..1000).all(|_| limits.admit().is_none()));
            let readers: Vec<_> = (0..READERS).map(|_| limits.reader().unwrap()).collect();
            assert!(limits.reader().is_none());
            drop((pending, readers));
            assert_eq!(limits.pending.load(Ordering::Acquire), 0);
            assert_eq!(limits.readers.load(Ordering::Acquire), 0);
        }
    }

    #[test]
    fn size_boundaries_and_overflow() {
        let limits = Arc::new(Limits::new(usize::MAX));
        let lease = limits.admit().unwrap();
        assert!(lease.reserve_file(FILE_LIMIT));
        assert!(!lease.reserve_file(FILE_LIMIT + 1));
        assert!(!lease.reserve_file(usize::MAX));
        drop(lease);
        assert_eq!(limits.memory.load(Ordering::Acquire), 0);
        let limits = Arc::new(Limits::new(OVERHEAD + 120));
        let lease = limits.admit().unwrap();
        assert!(lease.reserve_file(10));
        assert!(!lease.reserve_file(11));
    }

    #[test]
    fn panic_and_cancel_release_worker_and_memory() {
        let limits = Arc::new(Limits::new(MEMORY_LIMIT));
        let lease = limits.admit().unwrap();
        assert!(lease.reserve_file(10));
        let worker_lease = lease.clone();
        let reader = limits.reader().unwrap();
        drop(lease); // Cancelled future; the worker must still own its allocation.
        assert_ne!(limits.memory.load(Ordering::Acquire), 0);
        assert!(
            std::panic::catch_unwind(move || {
                let _resources = (worker_lease, reader);
                panic!("synthetic read panic");
            })
            .is_err()
        );
        assert_eq!(limits.memory.load(Ordering::Acquire), 0);
        assert_eq!(limits.pending.load(Ordering::Acquire), 0);
        assert_eq!(limits.readers.load(Ordering::Acquire), 0);
    }

    #[test]
    fn concurrent_reservations_cannot_overbook() {
        let limits = Arc::new(Limits::new(OVERHEAD + 120));
        let barrier = Arc::new(std::sync::Barrier::new(16));
        let threads: Vec<_> = (0..16)
            .map(|_| {
                let (limits, barrier) = (limits.clone(), barrier.clone());
                std::thread::spawn(move || {
                    let lease = limits.admit().unwrap();
                    barrier.wait();
                    let admitted = lease.reserve_file(10);
                    barrier.wait();
                    admitted
                })
            })
            .collect();
        assert_eq!(threads.into_iter().map(|t| usize::from(t.join().unwrap())).sum::<usize>(), 1);
        assert_eq!(limits.memory.load(Ordering::Acquire), 0);
    }
}
