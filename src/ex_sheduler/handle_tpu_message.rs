use agave_scheduler_bindings::{
    MAX_TRANSACTIONS_PER_MESSAGE, TpuToPackMessage, SharableTransactionRegion,
    pack_message_flags, check_flags,
};
use agave_scheduling_utils::handshake::client::ClientWorkerSession;
use agave_scheduling_utils::transaction_ptr::TransactionPtr;
use agave_transaction_view::transaction_version::TransactionVersion;
use agave_transaction_view::transaction_view::SanitizedTransactionView;
use rts_alloc::Allocator;
use shaq::Consumer;
use std::collections::{HashMap, VecDeque};

use super::transaction_entry::TransactionEntry;
use super::utils::PubkeysPtr;

pub fn handle_tpu_messages(
    allocator: &Allocator,
    tpu_to_pack: &mut Consumer<TpuToPackMessage>,
    resolving_worker: &mut ClientWorkerSession,
    queue: &mut VecDeque<usize>,
    offset_to_entry: &mut HashMap<usize, TransactionEntry>,
) {
    tpu_to_pack.sync();

    let mut txs_to_resolve = Vec::with_capacity(MAX_TRANSACTIONS_PER_MESSAGE);

    while let Some(message) = tpu_to_pack.try_read() {
        // If at capacity, we will just drop the transaction here.
        let tx_ptr = unsafe {
            TransactionPtr::from_sharable_transaction_region(&message.transaction, allocator)
        };
        if queue.len() >= QUEUE_CAPACITY {
            unsafe { tx_ptr.free(allocator) };
            continue;
        }

        let Ok(view) = SanitizedTransactionView::try_new_sanitized(
            unsafe {
                TransactionPtr::from_sharable_transaction_region(&message.transaction, allocator)
            },
            true,
        ) else {
            unsafe { tx_ptr.free(allocator) };
            continue;
        };

        // V0 transactions get sent off for resolution.
        if matches!(view.version(), TransactionVersion::V0) {
            txs_to_resolve.push(unsafe { tx_ptr.to_sharable_transaction_region(allocator) });
            if txs_to_resolve.len() == MAX_TRANSACTIONS_PER_MESSAGE {
                send_resolve_requests(allocator, resolving_worker, &txs_to_resolve);
                txs_to_resolve.clear();
            }
            continue;
        }

        // Non-ALT transactions go immediately to queue.
        queue.push_back(message.transaction.offset);
        offset_to_entry.insert(
            message.transaction.offset,
            TransactionEntry {
                sharable_transaction: SharableTransactionRegion {
                    offset: message.transaction.offset,
                    length: message.transaction.length,
                },
                view,
                loaded_addresses: None,
            },
        );
    }

    if !txs_to_resolve.is_empty() {
        send_resolve_requests(allocator, resolving_worker, &txs_to_resolve);
        txs_to_resolve.clear();
    }

    tpu_to_pack.finalize();
}



pub fn send_resolve_requests(
    allocator: &Allocator,
    worker: &mut ClientWorkerSession,
    txs: &[SharableTransactionRegion],
) {
    let Some(batch_ptr) = allocator.allocate(core::mem::size_of_val(txs) as u32) else {
        panic!("failed to allocate");
    };
    unsafe { core::ptr::copy_nonoverlapping(txs.as_ptr(), batch_ptr.cast().as_ptr(), txs.len()) };

    worker.pack_to_worker.sync();
    if worker
        .pack_to_worker
        .try_write(PackToWorkerMessage {
            flags: pack_message_flags::CHECK | check_flags::LOAD_ADDRESS_LOOKUP_TABLES,
            max_working_slot: u64::MAX,
            batch: SharableTransactionBatchRegion {
                num_transactions: txs.len() as u8,
                transactions_offset: unsafe { allocator.offset(batch_ptr) },
            },
        })
        .is_err()
    {
        panic!("agave too far behind");
    }
    worker.pack_to_worker.commit();
}