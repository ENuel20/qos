use std::time::Duration;

use agave_scheduler_bindings::{
    SharableTransactionBatchRegion, TransactionResponseRegion, WorkerToPackMessage, processed_codes::{self, PROCESSED},
};

use crate::agave::SLEEP_WORKER_TO_PACK;

pub fn send_message(
    producer: &mut shaq::Producer<WorkerToPackMessage>,
    batch: SharableTransactionBatchRegion,
    processed_code: u8,
    responses: TransactionResponseRegion,
) -> bool {
    producer.sync();

    let Some(slot) = (unsafe { producer.reserve() })else {
        return false;
    };

    unsafe {
        slot.write(WorkerToPackMessage {
            batch,
            processed_code,
            responses,
        });
    }

    producer.commit();
    true
}

/// Send a response back through the worker_to_pack queue
pub fn send_response(
    producer: &mut shaq::Producer<WorkerToPackMessage>,
    batch: SharableTransactionBatchRegion,
) -> bool {
    let num_transactions = batch.num_transactions;
    let responses = TransactionResponseRegion {
        tag: 0,
        num_transaction_responses: num_transactions,
        transaction_responses_offset: 0,
    };
    let processed = 0x01;

    println!(
        "\n[Agave] send -> worker_to_pack :
            processed {}
            num_transactions {}
            responses.tag {}
            responses.num_transaction_responses {}
            responses.transaction_responses_offset {}",
        processed,
        num_transactions,
        responses.tag,
        responses.num_transaction_responses,
        responses.transaction_responses_offset,
    );

    let sent = send_message(producer, batch, processed, responses);

    if SLEEP_WORKER_TO_PACK > 0 {
        std::thread::sleep(Duration::from_millis(SLEEP_WORKER_TO_PACK));
    };
    sent
}
