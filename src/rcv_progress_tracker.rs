use {
    agave_scheduler_bindings::ProgressMessage,
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
    mut consumer: shaq::Consumer<ProgressMessage>,
) -> JoinHandle<()> {
    std::thread::spawn(move || {
        while !exit.load(Ordering::Relaxed) {
            consumer.sync();
            if let Some(slot) = consumer.try_read() {
                let msg = unsafe {
                    let message = slot.as_ref();
                    message
                };

                println!(
                    "\n[Ext. Scheduler] progress_tracker -> rcv :  current_slot {} next_leader_slot {} current_slot_progress {}",
                    msg.current_slot, msg.next_leader_slot, msg.current_slot_progress
                );
            };
            consumer.finalize();
        }
    })
}
