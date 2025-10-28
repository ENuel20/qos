use {
    crate::shared::{PackToWorkerReceiver, SendAllocator},
    agave_scheduler_bindings::{
        PackToWorkerMessage, SharableTransactionBatchRegion, SharableTransactionRegion,
    },
    crossbeam_channel::Receiver,
    rts_alloc::Allocator,
    solana_perf::packet::to_packet_batches,
    solana_transaction::Transaction,
    std::{
        ptr::NonNull,
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
        thread::JoinHandle,
    },
};

pub fn spawn(
    exit: Arc<AtomicBool>,
    worker_id: usize,
    receiver: Receiver<Arc<PackToWorkerReceiver>>,
    allocators: Arc<Vec<SendAllocator>>,
    mut producer: shaq::Producer<PackToWorkerMessage>,
) -> JoinHandle<()> {
    std::thread::Builder::new()
    .name(format!("prdcr_pack_to_worker-{}", worker_id))
    .spawn(move || {
    while !exit.load(Ordering::Relaxed) {
        crossbeam_channel::select! {
            recv(receiver) -> msg => {
                match msg {
                    Ok(pack_to_worker_receiver) => {
                        handle_transaction_batch(
                            &allocators[0],
                            &mut producer,
                            pack_to_worker_receiver,
                        );
                    },
                    Err(err) => {
                        println!(
                            "All transaction regions receiver have been closed, exiting Ext Scheduler PackToWorker service. {:?}",
                            err.to_string()
                        );
                        // Senders have been dropped, exit the loop.
                        continue;
                    }
                }
            }
        };
    }
     }).unwrap()
}

fn handle_transaction_batch(
    allocator: &SendAllocator,
    producer: &mut shaq::Producer<PackToWorkerMessage>,
    pack_to_worker_receiver: Arc<PackToWorkerReceiver>,
) {
    let transactions = pack_to_worker_receiver.transactions.as_slice();
    allocator.clean_remote_free_lists();
    if transactions.is_empty() || transactions.len() > u8::MAX as usize {
        return;
    }
    let flags = pack_to_worker_receiver.flags;
    let max_execution_slot = pack_to_worker_receiver.max_execution_slot;

    let new_transaction_regions = allocate_packet_batches(allocator, transactions);

    let batch_size =
        std::mem::size_of::<SharableTransactionRegion>() * new_transaction_regions.len();

    producer.sync();

    let Some((allocated_ptr, pack_to_worker_message)) =
        allocate_and_reserve_message(allocator, producer, batch_size)
    else {
        println!("Failed to allocate/reserve message. Dropping the rest of the batch.");
        return;
    };

    let allocated_ptr_offset_in_allocator = unsafe { allocator.offset(allocated_ptr) };

    println!(
        "\n[Ext. Scheduler] send -> pack_to_worker :
            flags {}
            max_execution_slot {}
            transactions.len {}",
        pack_to_worker_receiver.flags,
        pack_to_worker_receiver.max_execution_slot,
        pack_to_worker_receiver.transactions.len(),
    );
    unsafe {
        copy_transactions_and_populate_message(
            new_transaction_regions.as_slice(),
            flags,
            max_execution_slot,
            allocated_ptr,
            allocated_ptr_offset_in_allocator,
            pack_to_worker_message,
        );
    }
    producer.commit();
}

fn allocate_packet_batches(
    allocator: &Allocator,
    transactions: &[Transaction],
) -> Vec<SharableTransactionRegion> {
    let mut new_transaction_regions = Vec::with_capacity(transactions.len());
    let packet_batches = to_packet_batches(transactions, transactions.len());

    'batch_loop: for batch in packet_batches.iter() {
        for packet in batch.iter() {
            let Some(packet_bytes) = packet.data(..) else {
                continue;
            };
            let packet_size = packet_bytes.len();

            let Some(allocated_ptr) = allocator.allocate(packet_size as u32) else {
                println!("Failed to allocate message. Dropping the rest of the batch.");
                break 'batch_loop;
            };

            let allocated_ptr_offset_in_allocator = unsafe { allocator.offset(allocated_ptr) };

            unsafe {
                allocated_ptr.copy_from_nonoverlapping(
                    NonNull::new(packet_bytes.as_ptr().cast_mut())
                        .expect("packet bytes must be non-null"),
                    packet_bytes.len(),
                );
            }

            let transaction = SharableTransactionRegion {
                offset: allocated_ptr_offset_in_allocator,
                length: packet_bytes.len() as u32,
            };
            new_transaction_regions.push(transaction);
        }
    }
    new_transaction_regions
}

fn allocate_and_reserve_message(
    allocator: &Allocator,
    producer: &mut shaq::Producer<PackToWorkerMessage>,
    batch_size: usize,
) -> Option<(NonNull<u8>, NonNull<PackToWorkerMessage>)> {
    let allocated_ptr = allocator.allocate(batch_size as u32)?;

    let Some(pack_to_worker_message) = producer.reserve() else {
        unsafe {
            allocator.free(allocated_ptr);
        }
        return None;
    };

    Some((allocated_ptr, pack_to_worker_message))
}

unsafe fn copy_transactions_and_populate_message(
    transactions: &[SharableTransactionRegion],
    flags: u16,
    max_execution_slot: u64,
    allocated_ptr: NonNull<u8>,
    allocated_ptr_offset_in_allocator: usize,
    pack_to_worker_message: NonNull<PackToWorkerMessage>,
) {
    let num_transactions = transactions.len();
    unsafe {
        std::ptr::copy_nonoverlapping(
            transactions.as_ptr(),
            allocated_ptr.as_ptr() as *mut SharableTransactionRegion,
            num_transactions,
        );
    }
    unsafe {
        pack_to_worker_message.write(PackToWorkerMessage {
            flags,
            max_execution_slot,
            batch: SharableTransactionBatchRegion {
                num_transactions: num_transactions as u8,
                transactions_offset: allocated_ptr_offset_in_allocator,
            },
        });
    }
}
