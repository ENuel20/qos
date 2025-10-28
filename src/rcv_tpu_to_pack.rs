use {
    crate::shared::PackToWorkerReceiver,
    agave_scheduler_bindings::{TpuToPackMessage, pack_message_flags},
    bincode::config::Configuration,
    crossbeam_channel::Sender,
    rts_alloc::Allocator,
    solana_transaction::Transaction,
    std::{
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
        thread::JoinHandle,
    },
};

pub fn spawn(
    exit: Arc<AtomicBool>,
    sender: Sender<Arc<PackToWorkerReceiver>>,
    allocator: Allocator,
    mut consumer: shaq::Consumer<TpuToPackMessage>,
) -> JoinHandle<()> {
    std::thread::spawn(move || {
        let mut transactions: Vec<Transaction> = Vec::new();
        while !exit.load(Ordering::Relaxed) {
            consumer.sync();
            if let Some(slot) = consumer.try_read() {
                let msg = unsafe {
                    let message = slot.as_ref();
                    let ptr = allocator.ptr_from_offset(message.transaction.offset as usize);
                    let data = core::slice::from_raw_parts(
                        ptr.as_ref(),
                        message.transaction.length as usize,
                    );
                    if let Ok((tx, _)) = bincode::serde::decode_from_slice::<
                        Transaction,
                        Configuration,
                    >(data, Configuration::default())
                    {
                        transactions.push(tx);
                    }
                    allocator.free(ptr);
                    message
                };

                println!(
                    "\n[Ext. Scheduler] tpu_to_pack --> rcv : {} {:?} ",
                    msg.flags, msg.src_addr
                );
                if transactions.len() >= 64 {
                    let msg = pack_to_workers_batch(transactions.as_slice());
                    let _ = sender.send(Arc::new(msg));
                    transactions.clear();
                }
            };
            consumer.finalize();
        }
    })
}

fn pack_to_workers_batch(txs: &[Transaction]) -> PackToWorkerReceiver {
    let items = txs
        .iter()
        .map(|tx| Transaction {
            signatures: tx.signatures.clone(),
            message: tx.message.clone(),
        })
        .collect();
    PackToWorkerReceiver {
        flags: pack_message_flags::RESOLVE,
        max_execution_slot: 32,
        transactions: Arc::new(items),
    }
}
