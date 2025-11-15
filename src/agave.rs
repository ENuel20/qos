use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::collections::{HashMap, VecDeque};
use std::thread;
use std::time::Duration;

use agave_scheduler_bindings::{
    PackToWorkerMessage, ProgressMessage, TpuToPackMessage, WorkerToPackMessage,
};
use agave_scheduling_utils::{
    handshake::server::Server,
    thread_aware_account_locks::ThreadAwareAccountLocks,
    transaction_ptr::{TransactionPtr, TransactionPtrBatch},
};
use rts_alloc::Allocator;

mod handle_worker_messages;
mod handle_progress_message;
mod handle_tpu_message;
mod transaction_entry;
mod utils;

use handle_worker_messages::handle_worker_messages;
use handle_progress_message::spawn_progress_thread;
use handle_tpu_message::handle_tpu_messages;
use transaction_entry::{clear_queue, TransactionEntry};
use utils::*;

const NUM_WORKERS: usize = 5;
const QUEUE_CAPACITY: usize = 100_000;
const SLOT_DISTANCE_THRESHOLD: u64 = 20;

fn main() {
    let path = "/tmp/agave_server.sock";

    let exit = Arc::new(AtomicBool::new(false));
    signal_hook::flag::register(signal_hook::consts::SIGINT, exit.clone())
        .expect("failed to register signal handler");

    // Create and bind server socket
    let mut server = Server::new(path).expect("failed to create server");

    println!("Server listening on {path}...");

    // Accept a client logon (handshake)
    let session = server.accept().expect("failed to accept client logon");
    println!("Client connected!");

    let allocator = &session.workers[0].allocator;

    // Shared server state
    let mut queue = VecDeque::with_capacity(QUEUE_CAPACITY);
    let mut account_locks = ThreadAwareAccountLocks::new(session.workers.len());
    let mut in_progress = vec![0; session.workers.len()];
    let mut offset_to_entry = HashMap::with_capacity(QUEUE_CAPACITY);

    // Spawn a progress thread
    let _progress_handle = spawn_progress_thread(
        exit.clone(),
        session.progress_tracker.clone(),
    );

    let mut is_leader = false;

    while !exit.load(Ordering::Relaxed) {
        // Handle TPU → Pack messages
        handle_tpu_messages(
            allocator,
            &mut session.tpu_to_pack.producer,
            &mut session.workers[NUM_WORKERS - 1],
            &mut queue,
            &mut offset_to_entry,
        );

        // Handle Worker → Pack messages
        for (worker_index, worker) in session.workers.iter_mut().enumerate() {
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

        // Handle progress messages
        let mut should_clear = false;
        if let Some((new_is_leader, slots_until_leader)) =
            handle_progress_message(&mut session.progress_tracker)
        {
            is_leader = new_is_leader;
            should_clear = slots_until_leader > SLOT_DISTANCE_THRESHOLD;
        }

        // Schedule transactions or clear queue
        if is_leader {
            schedule(
                allocator,
                &mut session.workers[..NUM_WORKERS - 1],
                &mut queue,
                &offset_to_entry,
                &mut account_locks,
                &mut in_progress,
            );
        } else if should_clear {
            clear_queue(allocator, &mut queue, &mut offset_to_entry);
        }

        // Optional sleep to reduce busy-loop CPU usage
        thread::sleep(Duration::from_millis(10));
    }

    println!("Server exiting cleanly.");
}
