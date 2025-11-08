use {
    crate::agave::SLEEP_PACK_TO_WORKER,
    agave_scheduler_bindings::{
        PackToWorkerMessage, SharableTransactionBatchRegion, SharableTransactionRegion,
    },
    bincode::config::Configuration,
    rts_alloc::Allocator,
    solana_transaction::Transaction,
    std::time::Duration,
};

/// Process a batch of transactions from pack_to_worker queue
pub fn process_transaction_batch(
    allocator: &Allocator,
    consumer: &mut shaq::Consumer<PackToWorkerMessage>,
) -> Option<SharableTransactionBatchRegion> {
    consumer.sync();
    let item = consumer.try_read()?;

    let batch = unsafe {
        let message = item;
        let txs_ptr = allocator.ptr_from_offset(message.batch.transactions_offset as usize);
        let txs = core::slice::from_raw_parts(
            txs_ptr.as_ptr() as *const SharableTransactionRegion,
            message.batch.num_transactions as usize,
        );

        for tx in txs {
            if let Some(txn) = decode_transaction(&allocator, tx) {
                println!("\n[Agave] pack_to_worker --> rcv : Processing transaction: {:?}", txn);
            }
        }

        SharableTransactionBatchRegion {
            num_transactions: message.batch.num_transactions,
            transactions_offset: message.batch.transactions_offset,
        }
    };

    consumer.finalize();
    if SLEEP_PACK_TO_WORKER > 0 {
        std::thread::sleep(Duration::from_millis(SLEEP_PACK_TO_WORKER));
    }
    Some(batch)
}

/// Decode a single transaction from the allocator
pub fn decode_transaction(
    allocator: &Allocator,
    tx_region: &SharableTransactionRegion,
) -> Option<Transaction> {
    let ptr = allocator.ptr_from_offset(tx_region.offset as usize);
    let data = unsafe { core::slice::from_raw_parts(ptr.as_ref(), tx_region.length as usize) };

    bincode::serde::decode_from_slice::<Transaction, Configuration>(
        data,
        bincode::config::standard(),
    )
    .ok()
    .map(|(txn, _size)| txn)
}
