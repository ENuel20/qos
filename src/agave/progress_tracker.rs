use {
    crate::agave::SLEEP_PROGRESS_TRACKER,
    agave_scheduler_bindings::ProgressMessage,
    rand::Rng,
    std::{
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
        thread::{JoinHandle, sleep},
        time::Duration,
    },
};

pub fn spwan(exit: Arc<AtomicBool>, mut queue: shaq::Producer<ProgressMessage>) -> JoinHandle<()> {
    std::thread::spawn(move || {
        let mut rng = rand::rng();
        while !exit.load(Ordering::Relaxed) {
            let msgs = create_dynamic_mgs(&mut rng);

            handle_progress_tracker(&mut queue, msgs);
            if SLEEP_PROGRESS_TRACKER > 0 {
                // Simulate processing time
                sleep(Duration::from_millis(SLEEP_PROGRESS_TRACKER));
            }
        }
    })
}

fn create_dynamic_mgs(rng: &mut impl Rng) -> Vec<ProgressMessage> {
    let num_txs = rng.random_range(1..=10);

    (0..num_txs)
        .map(|_| {
            let current_slot: u64 = rng.random_range(1..10_000);
            let next_leader_slot: u64 = rng.random_range(current_slot..10_001);
            let remaining_cost_units: u64 = rng.random_range(1_000_000..60_000_000);
            let current_slot_progress: u8 = rng.random_range(0..100);

            ProgressMessage {
                current_slot,
                next_leader_slot,
                remaining_cost_units,
                current_slot_progress,
            }
        })
        .collect()
}

fn handle_progress_tracker(
    producer: &mut shaq::Producer<ProgressMessage>,
    progress_tracker_batches: Vec<ProgressMessage>,
) {
    for progress_tracker in progress_tracker_batches.iter() {
        producer.sync();

        let Some(progress_tracker_message) = producer.reserve() else {
            continue;
        };

        unsafe {
            progress_tracker_message.write(ProgressMessage {
                current_slot: progress_tracker.current_slot,
                next_leader_slot: progress_tracker.next_leader_slot,
                remaining_cost_units: progress_tracker.remaining_cost_units,
                current_slot_progress: progress_tracker.current_slot_progress,
            });
        }
        producer.commit();
    }
}
