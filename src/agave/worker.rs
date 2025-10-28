use {
    crate::agave::{SLEEP_WORKER, pack_to_worker, worker_to_pack},
    agave_scheduler_bindings::{PackToWorkerMessage, WorkerToPackMessage},
    rts_alloc::Allocator,
    std::{
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
        thread::JoinHandle,
        time::Duration,
    },
};

pub struct Worker {
    pub allocator: Allocator,
    pub pack_to_worker: shaq::Consumer<PackToWorkerMessage>,
    pub worker_to_pack: shaq::Producer<WorkerToPackMessage>,
}

impl Worker {
    pub fn spawn(self, worker_id: usize, exit: Arc<AtomicBool>) -> JoinHandle<()> {
        std::thread::Builder::new()
            .name(format!("worker-{}", worker_id))
            .spawn(move || {
                let Worker {
                    allocator,
                    mut pack_to_worker,
                    mut worker_to_pack,
                } = self;

                println!("Worker {} started", worker_id);

                while !exit.load(Ordering::Relaxed) {
                    // Read from pack_to_worker queue
                    if let Some(batch) =
                        pack_to_worker::process_transaction_batch(&allocator, &mut pack_to_worker)
                    {
                        // Send response back via worker_to_pack
                        worker_to_pack::send_response(&mut worker_to_pack, batch);
                    }

                    if SLEEP_WORKER > 0 {
                        std::thread::sleep(Duration::from_millis(SLEEP_WORKER));
                    }
                }

                println!("Worker {} finished", worker_id);
            })
            .unwrap()
    }
}
