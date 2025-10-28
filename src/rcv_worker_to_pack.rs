use {
    crate::shared::SendAllocator,
    agave_scheduler_bindings::WorkerToPackMessage,
    std::{
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
        thread::JoinHandle,
    },
};

pub fn spwan(
    exit: Arc<AtomicBool>,
    worker_id: usize,
    _allocator: &SendAllocator,
    mut consumer: shaq::Consumer<WorkerToPackMessage>,
) -> JoinHandle<()> {
    std::thread::Builder::new()
        .name(format!("rcv_worker_to_pack-{}", worker_id))
        .spawn(move || {
            while !exit.load(Ordering::Relaxed) {
                consumer.sync();
                if let Some(slot) = consumer.try_read() {
                    let msg = unsafe {
                        let message = slot.as_ref();
                        message
                    };

                    println!(
                        "\n[Ext. Scheduler] worker_to_pack -> rcv :
                            processed {}
                            batch.num_transactions {}
                            batch.transactions_offset {}
                            responses.tag {}
                            responses.num_transaction_responses {}
                            responses.transaction_responses_offset {}",
                        msg.processed,
                        msg.batch.num_transactions,
                        msg.batch.transactions_offset,
                        msg.responses.tag,
                        msg.responses.num_transaction_responses,
                        msg.responses.transaction_responses_offset,
                    );
                };
                consumer.finalize();
            }
        })
        .unwrap()
}
