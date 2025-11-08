use {
    crate::agave::{progress_tracker, tpu_to_pack, worker::Worker},
    agave_scheduling_utils::handshake::server::Server,
    std::{
        path::Path,
        sync::{Arc, atomic::AtomicBool},
        thread::JoinHandle,
    },
    tempfile::NamedTempFile,
};

pub fn spawn(exit: Arc<AtomicBool>) -> (impl AsRef<Path>, JoinHandle<()>) {
    println!("starting agave server side!");

    let ipc = NamedTempFile::new().unwrap();
    std::fs::remove_file(ipc.path()).unwrap();
    let mut server = Server::new(ipc.path()).unwrap();

    let server_handle = std::thread::spawn(move || {
        let mut session = server.accept().unwrap();

        // Send a tpu_to_pack message.
        let tpu_to_pack_thd = tpu_to_pack::spawn(
            exit.clone(),
            session.tpu_to_pack.allocator,
            session.tpu_to_pack.producer,
        );
        // Send a progress_tracker message
        let progress_tracker_thd = progress_tracker::spawn(exit.clone(), session.progress_tracker);

        // Vector to store worker thread handles
        let mut worker_handles: Vec<JoinHandle<()>> = Vec::new();

        // Take ownership of workers using std::mem::take
        let workers = std::mem::take(&mut session.workers);

        // Spawn a thread for each worker
        for (i, worker_session) in workers.into_iter().enumerate() {
            let worker = Worker {
                allocator: worker_session.allocator,
                pack_to_worker: worker_session.pack_to_worker,
                worker_to_pack: worker_session.worker_to_pack,
            };
            let handle = worker.spawn(i, exit.clone());
            worker_handles.push(handle);
            println!("Started worker thread {}", i);
        }

        tpu_to_pack_thd.join().unwrap();
        progress_tracker_thd.join().unwrap();
        for (i, handle) in worker_handles.into_iter().enumerate() {
            handle.join().unwrap();
            println!("Worker {} thread finished", i);
        }
    });

    (ipc, server_handle)
}
