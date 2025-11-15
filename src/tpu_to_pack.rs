use {
    agave_scheduler_bindings::{
        SharableTransactionRegion, TpuToPackMessage, tpu_message_flags,
    },
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
        thread::{sleep, JoinHandle},
        time::Duration,
    },
};

/// Custom delay in milliseconds between produced batches.
/// You choose this, not SLEEP_TPU_TO_PACK.
const CUSTOM_SLEEP_MS: u64 = 3;   // <--- YOU CAN MODIFY THIS ANYTIME

/// Spawn the Agave TPU→Pack producer.
/// This pushes TpuToPackMessage through shared memory into the EX-scheduler.
pub fn spawn(
    exit: Arc<AtomicBool>,
    allocator: Allocator,
    mut producer: shaq::Producer<TpuToPackMessage>,
) -> JoinHandle<()> {
    let (genesis_config, mint_keypair) =
        solana_genesis_config::create_genesis_config(u64::MAX);

    std::thread::spawn(move || {
        let mut rng = rand::thread_rng();

        while !exit.load(Ordering::Relaxed) {
            // create test bank + generate TXs
            let (bank, _forks) = Bank::new_with_bank_forks_for_tests(&genesis_config);

            let txs = create_random_txs(&bank, &mint_keypair, &mut rng);
            let batches = Arc::new(txs_to_packet_batches(&txs));

            // send into shared-memory queue
            handle_packet_batches(&allocator, &mut producer, &batches);

            // custom sleep (replaces SLEEP_TPU_TO_PACK)
            if CUSTOM_SLEEP_MS > 0 {
                sleep(Duration::from_millis(CUSTOM_SLEEP_MS));
            }
        }
    })
}

//
// ---------------- HELPERS --------------------
//

fn create_random_txs(
    bank: &Bank,
    mint: &Keypair,
    rng: &mut impl Rng,
) -> Vec<Transaction> {
    let bh = bank.last_blockhash();
    let n = rng.gen_range(1..=10);

    (0..n)
        .map(|_| {
            create_transfer_with_priority(
                bank,
                mint,
                &Keypair::new(),
                &Pubkey::new_unique(),
                rng.gen_range(1000..10_000),
                rng.gen_range(100..5000),
                bh,
            )
        })
        .collect()
}

fn create_transfer_with_priority(
    bank: &Bank,
    mint: &Keypair,
    from: &Keypair,
    to: &Pubkey,
    lamports: u64,
    cu_price: u64,
    bh: Hash,
) -> Transaction {
    // fund the sender
    let rent = bank.get_minimum_balance_for_rent_exemption(0);
    let funding = solana_system_transaction::transfer(
        mint,
        &from.pubkey(),
        rent + 1_000_000,
        bank.last_blockhash(),
    );
    bank.process_transaction(&funding).unwrap();

    // real tx
    let ix_transfer = system_instruction::transfer(&from.pubkey(), to, lamports);
    let ix_cu = ComputeBudgetInstruction::set_compute_unit_price(cu_price);

    let msg = Message::new(&[ix_transfer, ix_cu], Some(&from.pubkey()));
    Transaction::new(&[from], msg, bh)
}

fn txs_to_packet_batches(txs: &[Transaction]) -> Vec<PacketBatch> {
    let mut batches = to_packet_batches(txs, txs.len());

    // mark first as a vote (like real TPU)
    if let Some(batch0) = batches.get_mut(0) {
        if let Some(packet0) = batch0.get_mut(0) {
            packet0.meta_mut().set_simple_vote(true);
        }
    }

    batches
}

fn handle_packet_batches(
    allocator: &Allocator,
    producer: &mut shaq::Producer<TpuToPackMessage>,
    batches: &Vec<PacketBatch>,
) {
    allocator.clean_remote_free_lists();
    producer.sync();

    'outer: for batch in batches {
        for packet in batch {
            let Some(bytes) = packet.data(..) else { continue };

            // allocate shared mem for tx bytes
            let Some((ptr, slot)) =
                allocate_slot(allocator, producer, bytes.len())
            else {
                println!("TPU→Pack: allocation failed, dropping rest of batch");
                break 'outer;
            };

            let offset = unsafe { allocator.offset(ptr) };

            unsafe {
                write_msg(bytes, packet.meta(), ptr, offset, slot);
            }
        }
    }

    producer.commit();
}

fn allocate_slot(
    allocator: &Allocator,
    producer: &mut shaq::Producer<TpuToPackMessage>,
    size: usize,
) -> Option<(NonNull<u8>, NonNull<TpuToPackMessage>)> {
    let ptr = allocator.allocate(size as u32)?;
    let Some(slot) = (unsafe { producer.reserve() }) else {
        unsafe { allocator.free(ptr) };
        return None;
    };
    Some((ptr, slot))
}

unsafe fn write_msg(
    bytes: &[u8],
    meta: &solana_packet::Meta,
    ptr: NonNull<u8>,
    offset: usize,
    slot: NonNull<TpuToPackMessage>,
) {
    // copy packet → shared memory
    unsafe { ptr.copy_from_nonoverlapping(
        NonNull::new(bytes.as_ptr().cast_mut()).unwrap(),
        bytes.len(),
    ) };

    // define region
    let region = SharableTransactionRegion {
        offset,
        length: bytes.len() as u32,
    };

    unsafe { slot.write(TpuToPackMessage {
        transaction: region,
        flags: flags_from_meta(meta.flags),
        src_addr: map_src_addr(meta.addr),
    }) };
}

fn flags_from_meta(flags: PacketFlags) -> u8 {
    let mut x = 0;
    if flags.contains(PacketFlags::SIMPLE_VOTE_TX) {
        x |= tpu_message_flags::IS_SIMPLE_VOTE;
    }
    if flags.contains(PacketFlags::FORWARDED) {
        x |= tpu_message_flags::FORWARDED;
    }
    if flags.contains(PacketFlags::FROM_STAKED_NODE) {
        x |= tpu_message_flags::FROM_STAKED_NODE;
    }
    x
}

fn map_src_addr(addr: IpAddr) -> [u8; 16] {
    match addr {
        IpAddr::V4(ip) => ip.to_ipv6_mapped().octets(),
        IpAddr::V6(ip) => ip.octets(),
    }
}
