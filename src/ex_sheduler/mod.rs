/// External scheduler module for Agave
///
/// This module provides a complete external scheduler implementation that integrates with Agave
/// for transaction processing and scheduling.

pub mod schedule;
pub mod transaction_entry;
pub mod handle_progress_message;
pub mod handle_tpu_message;
pub mod handle_worker_messages;
pub mod utils;

// Re-export public types and functions
pub use schedule::schedule;
pub use transaction_entry::{TransactionEntry, clear_queue};
pub use handle_progress_message::handle_progress_message;
pub use handle_tpu_message::handle_tpu_messages;
pub use handle_worker_messages::handle_worker_messages;
pub use utils::{PubkeysPtr, ExecutionResponsesPtr, CheckResponsesPtr};

// Re-export commonly used types from agave and utilities
pub use agave_scheduler_bindings::{
    MAX_TRANSACTIONS_PER_MESSAGE, PackToWorkerMessage, ProgressMessage,
    SharableTransactionBatchRegion, SharableTransactionRegion, TpuToPackMessage,
    pack_message_flags, processed_codes,
    worker_message_types,
};
pub use agave_scheduling_utils::{
    handshake::{
        ClientLogon, ClientSession,
        client::{ClientSession as ClientSessionType, ClientWorkerSession},
        logon_flags,
    },
    thread_aware_account_locks::{ThreadAwareAccountLocks, ThreadSet},
    transaction_ptr::{TransactionPtr, TransactionPtrBatch},
};
pub use agave_transaction_view::{
    transaction_version::TransactionVersion,
    transaction_view::SanitizedTransactionView,
};
pub use rts_alloc::Allocator;
pub use shaq::Consumer;
pub use solana_pubkey::Pubkey;

// Constants for the scheduler
pub const NUM_WORKERS: usize = 5;
pub const QUEUE_CAPACITY: usize = 100_000;
pub const SLOT_DISTANCE_THRESHOLD: u64 = 20;
