use {rts_alloc::Allocator, solana_transaction::Transaction, std::sync::Arc};

// SAFETY: Allocator uses internal synchronization for cross-thread usage
// This is safe because the allocator is designed to be used across threads
// via remote free lists
pub struct SendAllocator(Allocator);
unsafe impl Send for SendAllocator {}
unsafe impl Sync for SendAllocator {}

impl SendAllocator {
    pub fn new(allocator: Allocator) -> Self {
        SendAllocator(allocator)
    }
}

impl std::ops::Deref for SendAllocator {
    type Target = Allocator;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

pub struct PackToWorkerReceiver {
    pub transactions: Arc<Vec<Transaction>>,
    pub flags: u16,
    pub max_execution_slot: u64,
}
