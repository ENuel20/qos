mod schedule;
mod transaction_entry;
mod handle_progress_message;
mod handle_tpu_message;
mod handle_worker_messages;
mod utils;

use agave_scheduler_bindings::{
    MAX_TRANSACTIONS_PER_MESSAGE, PackToWorkerMessage, ProgressMessage,
    SharableTransactionBatchRegion, SharableTransactionRegion, TpuToPackMessage,
    pack_message_flags::{self, check_flags},
    processed_codes,
    worker_message_types::{
        self, not_included_reasons, parsing_and_sanitization_flags, resolve_flags,
    },
};
use agave_scheduling_utils::{
    handshake::{
        ClientLogon,
        client::{ClientSession, ClientWorkerSession},
        logon_flags,
    },
    thread_aware_account_locks::{ThreadAwareAccountLocks, ThreadSet},
    transaction_ptr::{TransactionPtr, TransactionPtrBatch},
};
use agave_transaction_view::{
    transaction_version::TransactionVersion, transaction_view::SanitizedTransactionView,
};
use rts_alloc::Allocator;
use shaq::Consumer;
use solana_pubkey::Pubkey;
use std::{
    collections::{HashMap, VecDeque},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use handle_tpu_message::handle_tpu_messages;
use handle_worker_messages::handle_worker_messages;
use handle_progress_message::handle_progress_message;
use schedule::schedule;
use transaction_entry::{clear_queue, TransactionEntry};
use utils::*;

const NUM_WORKERS: usize = 5;
const QUEUE_CAPACITY: usize = 100_000;
const SLOT_DISTANCE_THRESHOLD: u64 = 20;

fn main() {
    // Collect command line arguments
    let args: Vec<String> = std::env::args().collect();

    // Expect a single argument (besides the program name)
    if args.len() < 2 {
        eprintln!("Usage: {} <path>", args[0]);
        std::process::exit(1);
    }
    let path = &args[1];

    let exit = Arc::new(AtomicBool::new(false));
    signal_hook::flag::register(signal_hook::consts::SIGINT, exit.clone())
        .expect("failed to register signal handler");

    let ClientSession {
        allocators,
        mut tpu_to_pack,
        mut progress_tracker,
        mut workers,
    } = agave_scheduling_utils::handshake::client::connect(
        path,
        ClientLogon {
            worker_count: NUM_WORKERS,
            allocator_size: 30 * 1024 * 1024 * 1024,
            allocator_handles: 1,
            tpu_to_pack_capacity: 128 * 1024,
            progress_tracker_capacity: 20 * 64,
            pack_to_worker_capacity: 64 * 1024,
            worker_to_pack_capacity: 64 * 1024,
            flags: logon_flags::REROUTE_VOTES,
        },
        Duration::from_secs(2),
    )
    .expect("failed to connect to agave");
    let allocator = &allocators[0];

    let mut queue = VecDeque::with_capacity(QUEUE_CAPACITY);
    let mut account_locks = ThreadAwareAccountLocks::new(workers.len());
    let mut in_progress = vec![0; workers.len()];
    let mut offset_to_entry = HashMap::with_capacity(QUEUE_CAPACITY);

    let mut is_leader = false;
    while !exit.load(Ordering::Relaxed) {
        handle_tpu_messages(
            allocator,
            &mut tpu_to_pack,
            &mut workers[NUM_WORKERS - 1],
            &mut queue,
            &mut offset_to_entry,
        );

        for (worker_index, worker) in workers.iter_mut().enumerate() {
            handle_worker_messages(
                allocator,
                worker_index,
                worker,
                &mut queue,
                &mut offset_to_entry,
                &mut account_locks,
                &mut in_progress,
            );
        }

        let mut should_clear = false;
        if let Some((new_is_leader, slots_until_leader)) =
            handle_progress_message(&mut progress_tracker)
        {
            is_leader = new_is_leader;
            should_clear = slots_until_leader > SLOT_DISTANCE_THRESHOLD;
        }

        if is_leader {
            schedule(
                allocator,
                &mut workers[..NUM_WORKERS - 1],
                &mut queue,
                &offset_to_entry,
                &mut account_locks,
                &mut in_progress,
            );
        } else if should_clear {
            clear_queue(allocator, &mut queue, &mut offset_to_entry);
        }
    }

}  


    