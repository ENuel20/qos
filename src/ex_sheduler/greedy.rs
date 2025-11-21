// Greedy scheduler implementation integrated into the external scheduler module.
// This scheduler consumes the Agave client session directly and schedules TXs by
// a greedy priority-based policy.

use static_assertions::const_assert;
use crate::transaction_map::{TransactionMap, TransactionStateKey};
use agave_feature_set::FeatureSet;
use agave_scheduler_bindings::pack_message_flags::check_flags;
use agave_scheduler_bindings::worker_message_types::{
    self, fee_payer_balance_flags, parsing_and_sanitization_flags, resolve_flags,
    status_check_flags,
};
use agave_scheduler_bindings::{
    IS_LEADER, MAX_TRANSACTIONS_PER_MESSAGE, PackToWorkerMessage, ProgressMessage,
    SharableTransactionBatchRegion, SharableTransactionRegion, TpuToPackMessage,
    WorkerToPackMessage, pack_message_flags, processed_codes,
};
use agave_scheduling_utils::handshake::client::{ClientSession, ClientWorkerSession};
use agave_scheduling_utils::pubkeys_ptr::PubkeysPtr;
use agave_scheduling_utils::responses_region::CheckResponsesPtr;
use agave_scheduling_utils::transaction_ptr::{TransactionPtr, TransactionPtrBatch};
use agave_transaction_view::transaction_view::{SanitizedTransactionView, TransactionView};
use hashbrown::HashMap;
use min_max_heap::MinMaxHeap;
use rts_alloc::Allocator;
use solana_compute_budget_instruction::compute_budget_instruction_details;
use solana_cost_model::block_cost_limits::MAX_BLOCK_UNITS_SIMD_0256;
use solana_cost_model::cost_model::CostModel;
use solana_fee::FeeFeatures;
use solana_fee_structure::FeeBudgetLimits;
use solana_pubkey::Pubkey;
use solana_runtime_transaction::runtime_transaction::RuntimeTransaction;
use solana_svm_transaction::svm_message::SVMStaticMessage;
use solana_transaction::sanitized::MessageHash;

const UNCHECKED_CAPACITY: usize = 64 * 1024;
const CHECKED_CAPACITY: usize = 64 * 1024;
const STATE_CAPACITY: usize = UNCHECKED_CAPACITY + CHECKED_CAPACITY;

const TX_REGION_SIZE: usize = std::mem::size_of::<SharableTransactionRegion>();
const TX_BATCH_PER_MESSAGE: usize = TX_REGION_SIZE + std::mem::size_of::<PriorityId>();
const TX_BATCH_SIZE: usize = TX_BATCH_PER_MESSAGE * MAX_TRANSACTIONS_PER_MESSAGE;
const_assert!(TX_BATCH_SIZE < 4096);
const TX_BATCH_META_OFFSET: usize = TX_REGION_SIZE * MAX_TRANSACTIONS_PER_MESSAGE;

/// How many percentage points before the end should we aim to fill the block.
const BLOCK_FILL_CUTOFF: u8 = 20;

pub struct GreedyQueues {
    pub tpu_to_pack: shaq::Consumer<TpuToPackMessage>,
    pub progress_tracker: shaq::Consumer<ProgressMessage>,
    pub workers: Vec<ClientWorkerSession>,
}

pub struct GreedyScheduler {
    allocator: Allocator,

    progress: ProgressMessage,
    runtime: RuntimeState,
    unchecked: MinMaxHeap<PriorityId>,
    checked: MinMaxHeap<PriorityId>,
    state: TransactionMap,
    in_flight_cost: u32,
    schedule_locks: HashMap<Pubkey, bool>,
}

impl GreedyScheduler {
    #[must_use]
    pub fn new(
        ClientSession { mut allocators, tpu_to_pack, progress_tracker, workers }: ClientSession,
    ) -> (Self, GreedyQueues) {
        assert_eq!(allocators.len(), 1, "invalid number of allocators");

        (
            Self {
                allocator: allocators.remove(0),

                progress: ProgressMessage {
                    leader_state: 0,
                    current_slot: 0,
                    next_leader_slot: u64::MAX,
                    leader_range_end: u64::MAX,
                    remaining_cost_units: 0,
                    current_slot_progress: 0,
                },
                runtime: RuntimeState {
                    feature_set: FeatureSet::all_enabled(),
                    fee_features: FeeFeatures { enable_secp256r1_precompile: true },
                    lamports_per_signature: 5000,
                    burn_percent: 50,
                },
                unchecked: MinMaxHeap::with_capacity(UNCHECKED_CAPACITY),
                checked: MinMaxHeap::with_capacity(UNCHECKED_CAPACITY),
                state: TransactionMap::with_capacity(STATE_CAPACITY),
                in_flight_cost: 0,
                schedule_locks: HashMap::default(),
            },
            GreedyQueues { tpu_to_pack, progress_tracker, workers },
        )
    }

    pub fn poll(&mut self, queues: &mut GreedyQueues) {
        // Drain the progress tracker so we know which slot we're on.
        self.drain_progress(queues);
        let is_leader = self.progress.leader_state == IS_LEADER;

        // Drain responses from workers.
        self.drain_worker_responses(queues);

        // Ingest a bounded amount of new transactions.
        match is_leader {
            true => self.drain_tpu(queues, 128),
            false => self.drain_tpu(queues, 1024),
        }

        // Drain pending checks.
        self.schedule_checks(queues);

        // Schedule if we're currently the leader.
        if is_leader {
            self.schedule_execute(queues);
        }

        // TODO: Think about re-checking all TXs on slot roll (or at least
        // expired TXs). If we do this we should use a dense slotmap to make
        // iteration fast.
    }

    fn drain_progress(&mut self, queues: &mut GreedyQueues) {
        queues.progress_tracker.sync();
        while let Some(msg) = queues.progress_tracker.try_read() {
            self.progress = *msg;
        }
        queues.progress_tracker.finalize();
    }

    fn drain_worker_responses(&mut self, queues: &mut GreedyQueues) {
        for worker in &mut queues.workers {
            worker.worker_to_pack.sync();
            while let Some(msg) = worker.worker_to_pack.try_read() {
                match msg.processed_code {
                    processed_codes::MAX_WORKING_SLOT_EXCEEDED => {
                        // SAFETY
                        // - Trust Agave to not have modified/freed this pointer.
                        unsafe {
                            self.allocator
                                .free_offset(msg.responses.transaction_responses_offset);
                        }

                        continue;
                    }
                    processed_codes::PROCESSED => {}
                    _ => panic!(),
                }

                match msg.responses.tag {
                    worker_message_types::CHECK_RESPONSE => self.on_check(msg),
                    worker_message_types::EXECUTION_RESPONSE => self.on_execute(msg),
                    _ => panic!(),
                }
            }
        }
    }

    fn drain_tpu(&mut self, queues: &mut GreedyQueues, max: usize) {
        queues.tpu_to_pack.sync();

        let additional = std::cmp::min(queues.tpu_to_pack.len(), max);
        let shortfall = (self.checked.len() + additional).saturating_sub(UNCHECKED_CAPACITY);

        // NB: Technically we are evicting more than we need to because not all of
        // `additional` will parse correctly & thus have a priority.
        for _ in 0..shortfall {
            let id = self.unchecked.pop_min().unwrap();
            // SAFETY:
            // - Trust Agave to behave correctly and not free these transactions.
            // - We have not previously freed this transaction.
            unsafe { self.state.remove(&self.allocator, id.key) };
        }

        // TODO: Need to dedupe already seen transactions.

        for _ in 0..additional {
            let msg = queues.tpu_to_pack.try_read().unwrap();

            match Self::calculate_priority(&self.runtime, &self.allocator, msg) {
                Some((view, priority, cost)) => {
                    let key = self.state.insert(msg.transaction, view);
                    self.unchecked.push(PriorityId { priority, cost, key });
                }
                // SAFETY:
                // - Trust Agave to have correctly allocated & trenferred ownership of this
                //   transaction region to us.
                None => unsafe {
                    self.allocator.free_offset(msg.transaction.offset);
                },
            }
        }
    }

    fn schedule_checks(&mut self, queues: &mut GreedyQueues) {
        let worker = &mut queues.workers[0];
        worker.pack_to_worker.sync();

        // Loop until worker queue is filled or backlog is empty.
        let worker_capacity = worker.pack_to_worker.capacity();
        for _ in 0..worker_capacity {
            if self.unchecked.is_empty() {
                break;
            }

            worker
                .pack_to_worker
                .try_write(PackToWorkerMessage {
                    flags: pack_message_flags::CHECK
                        | check_flags::STATUS_CHECKS
                        | check_flags::LOAD_FEE_PAYER_BALANCE
                        | check_flags::LOAD_ADDRESS_LOOKUP_TABLES,
                    max_working_slot: self.progress.current_slot + 1,
                    batch: Self::collect_batch(&self.allocator, || {
                        self.unchecked
                            .pop_max()
                            .map(|id| (id, self.state[id.key].shared))
                    }),
                })
                .expect("failed to write check message to worker");
        }

        worker.pack_to_worker.commit();
    }

    fn schedule_execute(&mut self, queues: &mut GreedyQueues) {
        self.schedule_locks.clear();

        debug_assert_eq!(self.progress.leader_state, IS_LEADER);
        let budget_percentage =
            std::cmp::min(self.progress.current_slot_progress + BLOCK_FILL_CUTOFF, 100);
        // TODO: Would be ideal for the scheduler protocol to tell us the max block
        // units.
        let budget_limit = MAX_BLOCK_UNITS_SIMD_0256 * u64::from(budget_percentage) / 100;
        let cost_used = MAX_BLOCK_UNITS_SIMD_0256
            .saturating_sub(self.progress.remaining_cost_units)
            + u64::from(self.in_flight_cost);
        let mut budget_remaining = budget_limit.saturating_sub(cost_used);
        for worker in &mut queues.workers[1..] {
            if budget_remaining == 0 || self.checked.is_empty() {
                return;
            }

            // If the worker already has a pending job, don't give it any more.
            worker.pack_to_worker.sync();
            if !worker.pack_to_worker.is_empty() {
                continue;
            }

            let batch = Self::collect_batch(&self.allocator, || {
                self.checked
                    .pop_max()
                    .filter(|id| {
                        // Check if we can fit the TX within our budget.
                        if u64::from(id.cost) > budget_remaining {
                            self.checked.push(*id);

                            return false;
                        }

                        // Check if this transaction's read/write locks conflict with any
                        // pre-existing read/write locks.
                        let tx = &self.state[id.key];
                        if tx
                            .write_locks()
                            .any(|key| self.schedule_locks.insert(*key, true).is_some())
                            || tx.read_locks().any(|key| {
                                self.schedule_locks
                                    .insert(*key, false)
                                    .is_some_and(|writable| writable)
                            })
                        {
                            self.checked.push(*id);
                            budget_remaining = 0;

                            return false;
                        }

                        // Update the budget as we are scheduling this TX.
                        budget_remaining = budget_remaining.saturating_sub(u64::from(id.cost));

                        true
                    })
                    .map(|id| {
                        self.in_flight_cost += id.cost;

                        (id, self.state[id.key].shared)
                    })
            });

            // If we failed to schedule anything, don't send the batch.
            if batch.num_transactions == 0 {
                return;
            }

            // Write the next batch for the worker.
            worker
                .pack_to_worker
                .try_write(PackToWorkerMessage {
                    flags: pack_message_flags::EXECUTE,
                    max_working_slot: self.progress.current_slot + 1,
                    batch,
                })
                .unwrap();
            worker.pack_to_worker.commit();
        }
    }

    fn on_check(&mut self, msg: &WorkerToPackMessage) {
        // SAFETY:
        // - Trust Agave to have allocated the batch/responses properly & told us the
        //   correct size.
        // - Use the correct wrapper type (check response ptr).
        // - Don't duplicate wrappers so we cannot double free.
        let (batch, responses) = unsafe {
            (
                TransactionPtrBatch::<PriorityId>::from_sharable_transaction_batch_region(
                    &msg.batch,
                    &self.allocator,
                ),
                CheckResponsesPtr::from_transaction_response_region(
                    &msg.responses,
                    &self.allocator,
                ),
            )
        };
        assert_eq!(batch.len(), responses.len());

        for ((_, id), rep) in batch.iter().zip(responses.iter()) {
            let parsing_failed =
                rep.parsing_and_sanitization_flags == parsing_and_sanitization_flags::FAILED;
            let status_failed = rep.status_check_flags
                & !(status_check_flags::REQUESTED | status_check_flags::PERFORMED)
                != 0;
            if parsing_failed || status_failed {
                // SAFETY:
                // - TX was previously allocated with this allocator.
                // - Trust Agave to not free this TX while returning it to us.
                unsafe {
                    self.state.remove(&self.allocator, id.key);
                }

                continue;
            }

            // Sanity check the flags.
            assert_ne!(rep.status_check_flags & status_check_flags::REQUESTED, 0);
            assert_ne!(rep.status_check_flags & status_check_flags::PERFORMED, 0);
            assert_eq!(rep.resolve_flags, resolve_flags::REQUESTED | resolve_flags::PERFORMED);
            assert_eq!(
                rep.fee_payer_balance_flags,
                fee_payer_balance_flags::REQUESTED | fee_payer_balance_flags::PERFORMED
            );

            // Evict lowest priority if at capacity.
            if self.checked.len() == CHECKED_CAPACITY {
                let evicted = self.checked.pop_min().unwrap();
                // SAFETY
                // - We have not previously freed the underlying transaction/pubkey objects.
                unsafe { self.state.remove(&self.allocator, evicted.key) };
            }

            // Insert the new transaction (yes this may be lower priority then what
            // we just evicted but that's fine).
            self.checked.push(id);

            // Update the state to include the resolved pubkeys.
            //
            // SAFETY
            // - Trust Agave to have allocated the pubkeys properly & transferred ownership
            //   to us.
            if rep.resolved_pubkeys.num_pubkeys > 0 {
                self.state[id.key].resolved = Some(unsafe {
                    PubkeysPtr::from_sharable_pubkeys(&rep.resolved_pubkeys, &self.allocator)
                });
            }
        }

        // Free both containers.
        unsafe { batch.free() };
        unsafe { responses.free(&self.allocator) };
    }

    fn on_execute(&mut self, msg: &WorkerToPackMessage) {
        // SAFETY:
        // - Trust Agave to have allocated the batch/responses properly & told us the
        //   correct size.
        // - Don't duplicate wrapper so we cannot double free.
        let batch: TransactionPtrBatch<PriorityId> = unsafe {
            TransactionPtrBatch::from_sharable_transaction_batch_region(&msg.batch, &self.allocator)
        };

        // Remove in-flight costs and free all transactions.
        for (tx, id) in batch.iter() {
            self.in_flight_cost -= id.cost;

            // SAFETY:
            // - Trust Agave not to have already freed this transaction as we are the owner.
            unsafe { tx.free(&self.allocator) };
        }

        // Free the containers.
        unsafe { batch.free() };
        // SAFETY:
        // - Trust Agave to have allocated these responses properly.
        unsafe {
            self.allocator
                .free_offset(msg.responses.transaction_responses_offset);
        }
    }

    fn collect_batch(
        allocator: &Allocator,
        mut pop: impl FnMut() -> Option<(PriorityId, SharableTransactionRegion)>,
    ) -> SharableTransactionBatchRegion {
        // Allocate a batch that can hold all our transaction pointers.
        let transactions = allocator.allocate(TX_BATCH_SIZE as u32).unwrap();
        let transactions_offset = unsafe { allocator.offset(transactions) };

        // Get our two pointers to the TX region & meta region.
        let tx_ptr = allocator
            .ptr_from_offset(transactions_offset)
            .cast::<SharableTransactionRegion>();
        // SAFETY:
        // - Pointer is guaranteed to not overrun the allocation as we just created it
        //   with a sufficient size.
        let meta_ptr = unsafe {
            allocator
                .ptr_from_offset(transactions_offset)
                .byte_add(TX_BATCH_META_OFFSET)
                .cast::<PriorityId>()
        };

        // Fill in the batch with transaction pointers.
        let mut num_transactions = 0;
        while num_transactions < MAX_TRANSACTIONS_PER_MESSAGE {
            let Some((id, tx)) = pop() else {
                break;
            };

            // SAFETY:
            // - We have allocated the transaction batch to support at least
            //   `MAX_TRANSACTIONS_PER_MESSAGE`, we terminate the loop before we overrun the
            //   region.
            unsafe {
                tx_ptr.add(num_transactions).write(tx);
                meta_ptr.add(num_transactions).write(id);
            };

            num_transactions += 1;
        }

        SharableTransactionBatchRegion {
            num_transactions: num_transactions.try_into().unwrap(),
            transactions_offset,
        }
    }

    fn calculate_priority(
        runtime: &RuntimeState,
        allocator: &Allocator,
        msg: &TpuToPackMessage,
    ) -> Option<(TransactionView<true, TransactionPtr>, u64, u32)> {
        let tx = SanitizedTransactionView::try_new_sanitized(
            // SAFETY:
            // - Trust Agave to have allocated the shared transactin region correctly.
            // - `SanitizedTransactionView` does not free the allocation on drop.
            unsafe {
                TransactionPtr::from_sharable_transaction_region(&msg.transaction, allocator)
            },
            true,
        )
        .ok()?;
        let tx = RuntimeTransaction::try_from(tx, &runtime.feature_set, MessageHash::Sha256).ok()?;

        // Compute transaction cost.
        let compute_budget_limits =
            compute_budget_instruction_details::ComputeBudgetInstructionDetails::try_from(
                tx.program_instructions_iter(),
            )
            .ok()?
            .sanitize_and_convert_to_compute_budget_limits(&runtime.feature_set)
            .ok()?;
        let fee_budget_limits = FeeBudgetLimits::from(compute_budget_limits);
        let cost = CostModel::calculate_cost(&tx, &runtime.feature_set).sum();

        // Compute transaction reward.
        let fee_details = solana_fee::calculate_fee_details(
            &tx,
            false,
            runtime.lamports_per_signature,
            fee_budget_limits.prioritization_fee,
            runtime.fee_features,
        );
        let burn = fee_details
            .transaction_fee()
            .checked_mul(runtime.burn_percent)?
            / 100;
        let base_fee = fee_details.transaction_fee() - burn;
        let reward = base_fee.saturating_add(fee_details.prioritization_fee());

        // Compute priority.
        Some((
            tx.into_inner_transaction(),
            reward
                .saturating_mul(1_000_000)
                .saturating_div(cost.saturating_add(1)),
            // TODO: Is it possible to craft a TX that passes sanitization with a cost > u32::MAX?
            cost.try_into().unwrap(),
        ))
    }
}

struct RuntimeState {
    feature_set: FeatureSet,
    fee_features: FeeFeatures,
    lamports_per_signature: u64,
    burn_percent: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C)]
struct PriorityId {
    priority: u64,
    cost: u32,
    key: TransactionStateKey,
}

impl PartialOrd for PriorityId {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for PriorityId {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.priority
            .cmp(&other.priority)
            .then_with(|| self.cost.cmp(&other.cost))
            .then_with(|| self.key.cmp(&other.key))
    }
}


