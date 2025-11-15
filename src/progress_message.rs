use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::thread;
use std::time::Duration;

use agave_scheduler_bindings::{
    ProgressMessage, IS_LEADER, IS_NOT_LEADER,
};
use shaq::Producer; // This is the queue type used in scheduler bindings

/// Spawns a thread that continually sends ProgressMessage updates
/// to the external scheduler.  
///
/// `exit` – atomic boolean for clean shutdown  
/// `mut producer` – the progress_tracker producer queue  
///
pub fn spawn_progress_thread(
    exit: Arc<AtomicBool>,
    mut producer: Producer<ProgressMessage>,
) -> thread::JoinHandle<()> {
    thread::Builder::new()
        .name("AgaveProgressThread".into())
        .spawn(move || {
            let mut current_slot: u64 = 1;
            let mut leader_range_start = 10;
            let mut leader_range_end = 12;

            loop {
                if exit.load(Ordering::Relaxed) {
                    println!("Progress thread exiting cleanly");
                    break;
                }

                // Compute leader state
                let is_leader_now =
                    (current_slot >= leader_range_start && current_slot <= leader_range_end);

                // Compute next leader slot
                let next_leader_slot = if is_leader_now {
                    // If already leader, next is AFTER this range
                    leader_range_end + 5
                } else if current_slot < leader_range_start {
                    leader_range_start
                } else {
                    // Past the range; schedule a new one
                    current_slot + 5
                };

                // Progress through slot (0–100%)
                let slot_progress = ((current_slot % 10) as u8) * 10;

                let msg = ProgressMessage {
                    leader_state: if is_leader_now { IS_LEADER } else { IS_NOT_LEADER },
                    current_slot,
                    next_leader_slot,
                    leader_range_end,
                    remaining_cost_units: if is_leader_now { 1_000_000 } else { 0 },
                    current_slot_progress: slot_progress,
                };

                // Send into shared memory ring queue
                if producer.try_write(msg).is_err() {
                    // queue full → drop (same as Agave does)
                    // You may print debug, but Agave normally wouldn't
                }

                // fake slot time
                thread::sleep(Duration::from_millis(100));

                current_slot += 1;

                // Occasionally move the leader range forward
                if current_slot > leader_range_end + 10 {
                    leader_range_start = current_slot + 3;
                    leader_range_end = leader_range_start + 2;
                }
            }
        })
        .expect("Failed to spawn progress thread")
}
