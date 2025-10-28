use {
    crate::agave::SLEEP_TPU_TO_PACK,
    agave_scheduler_bindings::{SharableTransactionRegion, TpuToPackMessage, tpu_message_flags},
    rand::Rng,
    rts_alloc::Allocator,
    solana_compute_budget_interface::ComputeBudgetInstruction,
    solana_keypair::Keypair,
    solana_message::Message,
    solana_packet::PacketFlags,
    solana_perf::packet::{PacketBatch, to_packet_batches},
    solana_pubkey::Pubkey,
    solana_runtime::bank::Bank,
    solana_sdk::hash::Hash,
    solana_signer::Signer,
    solana_system_interface::instruction as system_instruction,
    solana_transaction::Transaction,
    std::{
        net::IpAddr,
        ptr::NonNull,
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
        thread::{JoinHandle, sleep},
        time::Duration,
    },
};

pub fn spwan(
    exit: Arc<AtomicBool>,
    allocator: Allocator,
    mut queue: shaq::Producer<TpuToPackMessage>,
) -> JoinHandle<()> {
    let (genesis_config, mint_keypair) = solana_genesis_config::create_genesis_config(u64::MAX);

    std::thread::spawn(move || {
        let mut rng = rand::rng();
        while !exit.load(Ordering::Relaxed) {
            let (bank, _bank_fork) = Bank::new_with_bank_forks_for_tests(&genesis_config);
            let txs = create_dynamic_txs(&bank, &mint_keypair, &mut rng);
            let packet_batches = txs_to_banking_packet_batch(&txs);

            handle_packet_batches(&allocator, &mut queue, Arc::clone(&packet_batches));
            if SLEEP_TPU_TO_PACK > 0 {
                // Simulate processing time
                sleep(Duration::from_millis(SLEEP_TPU_TO_PACK));
            }
        }
    })
}

fn create_and_fund_prioritized_transfer(
    bank: &Bank,
    mint_keypair: &Keypair,
    from_keypair: &Keypair,
    to_pubkey: &Pubkey,
    lamports: u64,
    compute_unit_price: u64,
    recent_blockhash: Hash,
) -> Transaction {
    {
        let min_balance = bank.get_minimum_balance_for_rent_exemption(0);

        let transfer = solana_system_transaction::transfer(
            mint_keypair,
            &from_keypair.pubkey(),
            min_balance + 1_000_000,
            bank.last_blockhash(),
        );
        bank.process_transaction(&transfer).unwrap();
    }

    let transfer = system_instruction::transfer(&from_keypair.pubkey(), to_pubkey, lamports);
    let prioritization = ComputeBudgetInstruction::set_compute_unit_price(compute_unit_price);
    let message = Message::new(&[transfer, prioritization], Some(&from_keypair.pubkey()));
    Transaction::new(&vec![from_keypair], message, recent_blockhash)
}

fn create_dynamic_txs(bank: &Bank, mint_keypair: &Keypair, rng: &mut impl Rng) -> Vec<Transaction> {
    let last_blockhash = bank.last_blockhash();
    let num_txs = rng.random_range(1..=10);

    (0..num_txs)
        .map(|_| {
            let lamports = rng.random_range(1000..10000);
            let compute_unit_price = rng.random_range(100..5000);

            create_and_fund_prioritized_transfer(
                bank,
                mint_keypair,
                &Keypair::new(),
                &Pubkey::new_unique(),
                lamports,
                compute_unit_price,
                last_blockhash,
            )
        })
        .collect()
}

fn txs_to_banking_packet_batch(txs: &[Transaction]) -> Arc<Vec<PacketBatch>> {
    let mut packet_batches = to_packet_batches(txs, txs.len());
    packet_batches[0]
        .get_mut(0)
        .unwrap()
        .meta_mut()
        .set_simple_vote(true);
    Arc::new(packet_batches)
}

fn handle_packet_batches(
    allocator: &Allocator,
    producer: &mut shaq::Producer<TpuToPackMessage>,
    packet_batches: Arc<Vec<PacketBatch>>,
) {
    allocator.clean_remote_free_lists();

    producer.sync();

    'batch_loop: for batch in packet_batches.iter() {
        for packet in batch.iter() {
            let Some(packet_bytes) = packet.data(..) else {
                continue;
            };
            let packet_size = packet_bytes.len();

            let Some((allocated_ptr, tpu_to_pack_message)) =
                allocate_and_reserve_message(allocator, producer, packet_size)
            else {
                println!("Failed to allocate/reserve message. Dropping the rest of the batch.");
                break 'batch_loop;
            };

            let allocated_ptr_offset_in_allocator = unsafe { allocator.offset(allocated_ptr) };

            unsafe {
                copy_packet_and_populate_message(
                    packet_bytes,
                    packet.meta(),
                    allocated_ptr,
                    allocated_ptr_offset_in_allocator,
                    tpu_to_pack_message,
                );
            }
        }
    }
    producer.commit();
}

fn allocate_and_reserve_message(
    allocator: &Allocator,
    producer: &mut shaq::Producer<TpuToPackMessage>,
    packet_size: usize,
) -> Option<(NonNull<u8>, NonNull<TpuToPackMessage>)> {
    let allocated_ptr = allocator.allocate(packet_size as u32)?;

    let Some(tpu_to_pack_message) = producer.reserve() else {
        unsafe {
            allocator.free(allocated_ptr);
        }
        return None;
    };

    Some((allocated_ptr, tpu_to_pack_message))
}

unsafe fn copy_packet_and_populate_message(
    packet_bytes: &[u8],
    packet_meta: &solana_packet::Meta,
    allocated_ptr: NonNull<u8>,
    allocated_ptr_offset_in_allocator: usize,
    tpu_to_pack_message: NonNull<TpuToPackMessage>,
) {
    unsafe {
        allocated_ptr.copy_from_nonoverlapping(
            NonNull::new(packet_bytes.as_ptr().cast_mut()).expect("packet bytes must be non-null"),
            packet_bytes.len(),
        );
    }

    let transaction = SharableTransactionRegion {
        offset: allocated_ptr_offset_in_allocator,
        length: packet_bytes.len() as u32,
    };
    let tpu_message_flags = flags_from_meta(packet_meta.flags);

    let src_addr = map_src_addr(packet_meta.addr);

    unsafe {
        tpu_to_pack_message.write(TpuToPackMessage {
            transaction,
            flags: tpu_message_flags,
            src_addr,
        });
    }
}

fn flags_from_meta(flags: PacketFlags) -> u8 {
    let mut tpu_message_flags = 0;

    if flags.contains(PacketFlags::SIMPLE_VOTE_TX) {
        tpu_message_flags |= tpu_message_flags::IS_SIMPLE_VOTE;
    }
    if flags.contains(PacketFlags::FORWARDED) {
        tpu_message_flags |= tpu_message_flags::FORWARDED;
    }
    if flags.contains(PacketFlags::FROM_STAKED_NODE) {
        tpu_message_flags |= tpu_message_flags::FROM_STAKED_NODE;
    }

    tpu_message_flags
}

fn map_src_addr(addr: IpAddr) -> [u8; 16] {
    match addr {
        IpAddr::V4(ipv4) => ipv4.to_ipv6_mapped().octets(),
        IpAddr::V6(ipv6) => ipv6.octets(),
    }
}
