use agave_scheduler_bindings::{
    MAX_TRANSACTIONS_PER_MESSAGE, PackToWorkerMessage, SharableTransactionBatchRegion,
    SharableTransactionRegion, pack_message_flags,
};
use agave_scheduling_utils::handshake::client::ClientWorkerSession;
use agave_scheduling_utils::thread_aware_account_locks::{ThreadAwareAccountLocks, ThreadSet};
use rts_alloc::Allocator;
use std::collections::{HashMap, VecDeque};

use super::transaction_entry::TransactionEntry;

pub fn schedule(
    allocator: &Allocator,
    workers: &mut [ClientWorkerSession],
    queue: &mut VecDeque<usize>,
    offset_to_entry: &HashMap<usize, TransactionEntry>,
    account_locks: &mut ThreadAwareAccountLocks,
    in_progress: &mut [u64],
) {
    if queue.is_empty() {
        return;
    }

    workers
        .iter_mut()
        .for_each(|worker| worker.pack_to_worker.sync());

    let mut attempted = 0;
    let mut working_batches = (0..workers.len()).map(|_| None).collect::<Vec<_>>();
    const MAX_ATTEMPTED: usize = 10_000;

    while attempted < MAX_ATTEMPTED {
        let Some(offset) = queue.pop_front() else {
            break;
        };
        let entry = offset_to_entry.get(&offset).unwrap();

        attempted += 1;

        let Ok(thread_index) = account_locks.try_lock_accounts(
            entry.writable_account_keys(),
            entry.readonly_account_keys(),
            ThreadSet::any(workers.len()),
            |thread_set| {
                thread_set
                    .contained_threads_iter()
                    .min_by(|a, b| in_progress[*a].cmp(&in_progress[*b]))
                    .unwrap()
            },
        ) else {
            queue.push_back(offset);
            continue;
        };

        let working_batch = &mut working_batches[thread_index];
        if working_batch.is_none() {
            // allocate a max-sized batch, and we just push in there.
            let batch_region = allocator
                .allocate(
                    (core::mem::size_of::<SharableTransactionRegion>()
                        * MAX_TRANSACTIONS_PER_MESSAGE) as u32,
                )
                .expect("failed to allocate");
            *working_batch = Some(PackToWorkerMessage {
                flags: pack_message_flags::EXECUTE,
                max_working_slot: u64::MAX,
                batch: SharableTransactionBatchRegion {
                    num_transactions: 0,
                    transactions_offset: unsafe { allocator.offset(batch_region) },
                },
            });
        }

        let Some(message) = working_batch.as_mut() else {
            account_locks.unlock_accounts(
                entry.writable_account_keys(),
                entry.readonly_account_keys(),
                thread_index,
            );
            queue.push_back(offset);
            continue;
        };

        unsafe {
            allocator
                .ptr_from_offset(message.batch.transactions_offset)
                .cast::<SharableTransactionRegion>()
                .add(usize::from(message.batch.num_transactions))
                .write(SharableTransactionRegion {
                    offset: entry.sharable_transaction.offset,
                    length: entry.sharable_transaction.length,
                })
        };
        message.batch.num_transactions += 1;
        in_progress[thread_index] += 1;

        if usize::from(message.batch.num_transactions) >= MAX_TRANSACTIONS_PER_MESSAGE {
            let worker = &mut workers[thread_index];
            worker.pack_to_worker.sync();
            if worker
                .pack_to_worker
                .try_write(working_batch.take().expect("must be some"))
                .is_err()
            {
                panic!("agave too far behind")
            };
            worker.pack_to_worker.commit();
        };
    }

    for (working_batch, worker) in working_batches.into_iter().zip(workers.iter_mut()) {
        if let Some(working_batch) = working_batch {
            worker.pack_to_worker.sync();
            if worker.pack_to_worker.try_write(working_batch).is_err() {
                panic!("agave too far behind");
            }
            worker.pack_to_worker.commit();
        }
    }
}
