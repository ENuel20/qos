use {
    crate::shared::{PackToWorkerReceiver, SendAllocator},
    agave_scheduler_bindings::{PackToWorkerMessage, WorkerToPackMessage},
    agave_scheduling_utils::handshake::{ClientLogon, client::connect},
    crossbeam_channel::{Receiver, unbounded},
    std::{
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
        thread::JoinHandle,
        time::Duration,
    },
};

mod agave;
mod prdcr_pack_to_worker;
mod rcv_progress_tracker;
mod rcv_tpu_to_pack;
mod rcv_worker_to_pack;
mod shared;

pub struct Worker {
    pub allocators: Arc<Vec<SendAllocator>>,
    pub receiver: Receiver<Arc<PackToWorkerReceiver>>,
    pub pack_to_worker: shaq::Producer<PackToWorkerMessage>,
    pub worker_to_pack: shaq::Consumer<WorkerToPackMessage>,
}

impl Worker {
    pub fn spawn(self, worker_id: usize, exit: Arc<AtomicBool>) -> JoinHandle<()> {
        std::thread::Builder::new()
            .name(format!("worker-{}", worker_id))
            .spawn(move || {
                println!("Worker {} started", worker_id);
                let pack_to_worker_thd = prdcr_pack_to_worker::spawn(
                    exit.clone(),
                    worker_id,
                    self.receiver,
                    Arc::clone(&self.allocators),
                    self.pack_to_worker,
                );

                let worker_to_pack_allocator = &self.allocators[1];

                let worker_to_pack_thd = rcv_worker_to_pack::spwan(
                    exit.clone(),
                    worker_id,
                    worker_to_pack_allocator,
                    self.worker_to_pack,
                );

                println!("Worker {} finished", worker_id);
                pack_to_worker_thd.join().unwrap();
                worker_to_pack_thd.join().unwrap();
            })
            .unwrap()
    }
}

fn main() {
    println!("starting external scheduler!");
    let exit = Arc::new(AtomicBool::new(false));
    ctrlc::set_handler({
        let exit = exit.clone();
        move || exit.store(true, Ordering::Release)
    })
    .unwrap();

    let (ipc, server_handle) = agave::server::spwan(exit.clone());

    let session = connect(
        ipc,
        ClientLogon {
            worker_count: 4,
            allocator_size: 1024 * 1024 * 1024,
            allocator_handles: 3,
            tpu_to_pack_size: 65536 * 1024,
            progress_tracker_size: 16 * 1024,
            pack_to_worker_size: 1024 * 1024,
            worker_to_pack_size: 1024 * 1024,
        },
        Duration::from_secs(1),
    )
    .unwrap();

    let (pack_to_wrk_sender, pack_to_wrk_receiver) = unbounded::<Arc<PackToWorkerReceiver>>();

    let mut allocators_vec = session.allocators;
    let tpu_to_pack_allocator = allocators_vec.remove(0);
    // Receive tpu_to_pack message
    let tpu_to_pack_handle = rcv_tpu_to_pack::spawn(
        exit.clone(),
        pack_to_wrk_sender,
        tpu_to_pack_allocator,
        session.tpu_to_pack,
    );

    // Receive progress_tracker message.
    let progress_tracker_handle =
        rcv_progress_tracker::spawn(exit.clone(), session.progress_tracker);

    // Vector to store worker thread handles
    let mut worker_handles: Vec<JoinHandle<()>> = Vec::new();

    let worker_allocators: Vec<SendAllocator> = allocators_vec
        .into_iter()
        .take(2)
        .map(SendAllocator::new)
        .collect();
    let shared_allocators = Arc::new(worker_allocators);

    // Spawn a thread for each worker
    for (i, worker_session) in session.workers.into_iter().enumerate() {
        let worker = Worker {
            allocators: Arc::clone(&shared_allocators),
            receiver: pack_to_wrk_receiver.clone(),
            pack_to_worker: worker_session.pack_to_worker,
            worker_to_pack: worker_session.worker_to_pack,
        };
        let handle = worker.spawn(i, exit.clone());
        worker_handles.push(handle);
        println!("Started worker thread {}", i);
    }

    progress_tracker_handle.join().unwrap();
    tpu_to_pack_handle.join().unwrap();
    server_handle.join().unwrap();
    for (i, handle) in worker_handles.into_iter().enumerate() {
        handle.join().unwrap();
        println!("Worker {} thread finished", i);
    }
}
