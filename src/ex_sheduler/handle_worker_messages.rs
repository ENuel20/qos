use agave_scheduler_bindings::{
    processed_codes, worker_message_types,
    worker_message_types::{not_included_reasons, parsing_and_sanitization_flags, resolve_flags},
};
use agave_scheduling_utils::{
    handshake::client::ClientWorkerSession,
    thread_aware_account_locks::ThreadAwareAccountLocks,
    transaction_ptr::{TransactionPtr, TransactionPtrBatch},
};
use agave_transaction_view::transaction_view::SanitizedTransactionView;
use rts_alloc::Allocator;
use std::collections::{HashMap, VecDeque};

use super::transaction_entry::TransactionEntry;
use super::utils::{CheckResponsesPtr, ExecutionResponsesPtr, PubkeysPtr};

pub fn handle_worker_messages(
    allocator: &Allocator,
    worker_index: usize,
    worker: &mut ClientWorkerSession,
    queue: &mut VecDeque<usize>,
    offset_to_entry: &mut HashMap<usize, TransactionEntry>,
    account_locks: &mut ThreadAwareAccountLocks,
    in_progress: &mut [u64],
) {
    const QUEUE_CAPACITY: usize = 100_000;
    
    worker.worker_to_pack.sync();

    while let Some(message) = worker.worker_to_pack.try_read() {
        let batch = unsafe {
            TransactionPtrBatch::from_sharable_transaction_batch_region(&message.batch, allocator)
        };

        let processed = match message.processed_code {
            processed_codes::PROCESSED => true,
            processed_codes::MAX_WORKING_SLOT_EXCEEDED => false,
            processed_codes::INVALID => {
                panic!("We produced a message agave did not understand!");
            }
            _ => {
                panic!("agave produced a message we do not understand!")
            }
        };

        match message.responses.tag {
            worker_message_types::EXECUTION_RESPONSE => {
                if processed {
                    let execution_responses_ptr = unsafe {
                        ExecutionResponsesPtr::from_transaction_response_region(
                            &message.responses,
                            allocator,
                        )
                    };

                    // Unlock and push back into queue.
                    for (tx_ptr, response) in batch.iter().zip(execution_responses_ptr.iter()) {
                        let offset =
                            unsafe { tx_ptr.to_sharable_transaction_region(allocator).offset };
                        let entry = offset_to_entry.get(&offset).unwrap();
                        account_locks.unlock_accounts(
                            entry.writable_account_keys(),
                            entry.readonly_account_keys(),
                            worker_index,
                        );
                        in_progress[worker_index] -= 1;

                        // If rejected because of a cost-tracking error, we'll retry, but only if
                        // queue is also under capacity.
                        match response.not_included_reason {
                            not_included_reasons::WOULD_EXCEED_ACCOUNT_DATA_BLOCK_LIMIT
                            | not_included_reasons::WOULD_EXCEED_ACCOUNT_DATA_TOTAL_LIMIT
                            | not_included_reasons::WOULD_EXCEED_MAX_ACCOUNT_COST_LIMIT
                            | not_included_reasons::WOULD_EXCEED_MAX_BLOCK_COST_LIMIT
                            | not_included_reasons::WOULD_EXCEED_MAX_VOTE_COST_LIMIT
                                if queue.len() < QUEUE_CAPACITY =>
                            {
                                queue.push_back(offset);
                            }
                            _ => {
                                let entry = offset_to_entry.remove(&offset).unwrap();
                                if let Some(loaded_addresses) = entry.loaded_addresses {
                                    unsafe { loaded_addresses.free(allocator) };
                                }
                                unsafe { tx_ptr.free(allocator) };
                            }
                        };
                    }

                    execution_responses_ptr.free_wrapper();
                } else {
                    // Unlock and push back into queue IF there is room.
                    for tx_ptr in batch.iter() {
                        let offset =
                            unsafe { tx_ptr.to_sharable_transaction_region(allocator).offset };
                        let entry = offset_to_entry.get(&offset).unwrap();
                        account_locks.unlock_accounts(
                            entry.writable_account_keys(),
                            entry.readonly_account_keys(),
                            worker_index,
                        );
                        in_progress[worker_index] -= 1;

                        if queue.len() >= QUEUE_CAPACITY {
                            let entry = offset_to_entry.remove(&offset).unwrap();
                            if let Some(loaded_addresses) = entry.loaded_addresses {
                                unsafe { loaded_addresses.free(allocator) };
                            }
                            unsafe { tx_ptr.free(allocator) };
                            continue;
                        }
                        queue.push_back(offset);
                    }
                }
            }
            worker_message_types::CHECK_RESPONSE => {
                assert!(processed);
                let responses = unsafe {
                    CheckResponsesPtr::from_transaction_response_region(
                        &message.responses,
                        allocator,
                    )
                };

                for (tx_ptr, response) in batch.iter().zip(responses.iter()) {
                    // FIFO-demo only requests resolving of pubkeys,
                    // just assert that other flags are not set.
                    assert_eq!(response.fee_payer_balance_flags, 0);
                    assert_eq!(response.status_check_flags, 0);

                    // If the transaction failed to parse/sanitize drop it here.
                    // If resolving failed, we drop.
                    if response.parsing_and_sanitization_flags
                        & parsing_and_sanitization_flags::FAILED
                        != 0
                        || response.resolve_flags & resolve_flags::FAILED != 0
                    {
                        unsafe { tx_ptr.free(allocator) };
                        continue;
                    }

                    // Ensure we had successful resolution.
                    assert_eq!(
                        response.resolve_flags,
                        resolve_flags::REQUESTED | resolve_flags::PERFORMED
                    );

                    let loaded_addresses = if response.resolved_pubkeys.num_pubkeys != 0 {
                        Some(unsafe {
                            PubkeysPtr::from_sharable_pubkeys(&response.resolved_pubkeys, allocator)
                        })
                    } else {
                        None
                    };

                    // If queue is full we drop it.
                    if queue.len() >= QUEUE_CAPACITY {
                        if let Some(addresses) = loaded_addresses {
                            unsafe { addresses.free(allocator) };
                        }
                        unsafe { tx_ptr.free(allocator) };
                        continue;
                    }

                    let offset = unsafe { tx_ptr.to_sharable_transaction_region(allocator) }.offset;
                    let entry = TransactionEntry {
                        sharable_transaction: unsafe {
                            tx_ptr.to_sharable_transaction_region(allocator)
                        },
                        view: SanitizedTransactionView::try_new_sanitized(tx_ptr, true)
                            .expect("message corrupted"),
                        loaded_addresses,
                    };

                    queue.push_back(offset);
                    offset_to_entry.insert(offset, entry);
                }

                responses.free_wrapper();
            }
            _ => {
                panic!("agave sent a message with tag we do not understand!");
            }
        }
        unsafe {
            allocator.free(allocator.ptr_from_offset(message.batch.transactions_offset));
        }
    }

    worker.worker_to_pack.finalize();
}
