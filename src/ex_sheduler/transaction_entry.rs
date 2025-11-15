use agave_scheduler_bindings::SharableTransactionRegion;
use agave_transaction_view::transaction_view::SanitizedTransactionView;
use agave_scheduling_utils::transaction_ptr::TransactionPtr;
use rts_alloc::Allocator;
use solana_pubkey::Pubkey;
use std::collections::{HashMap, VecDeque};

use super::utils::PubkeysPtr;

pub struct TransactionEntry {
    sharable_transaction: SharableTransactionRegion,
    view: SanitizedTransactionView<TransactionPtr>,
    loaded_addresses: Option<PubkeysPtr>,
}

impl TransactionEntry {
    pub fn writable_account_keys(&self) -> impl Iterator<Item = &Pubkey> + Clone {
        self.view
            .static_account_keys()
            .iter()
            .chain(
                self.loaded_addresses
                    .iter()
                    .flat_map(|loaded_addresses| loaded_addresses.as_slice().iter()),
            )
            .enumerate()
            .filter(|(index, _)| self.requested_write(*index as u8))
            .map(|(_index, key)| key)
    }

    pub fn readonly_account_keys(&self) -> impl Iterator<Item = &Pubkey> + Clone {
        self.view
            .static_account_keys()
            .iter()
            .chain(
                self.loaded_addresses
                    .iter()
                    .flat_map(|loaded_addresses| loaded_addresses.as_slice().iter()),
            )
            .enumerate()
            .filter(|(index, _)| !self.requested_write(*index as u8))
            .map(|(_index, key)| key)
    }

    #[inline(always)]
    fn requested_write(&self, index: u8) -> bool {
        if index >= self.view.num_static_account_keys() {
            let loaded_address_index = index.wrapping_sub(self.view.num_static_account_keys());
            loaded_address_index < self.view.total_writable_lookup_accounts() as u8
        } else {
            index
                < self
                    .view
                    .num_signatures()
                    .wrapping_sub(self.view.num_readonly_signed_static_accounts())
                || (index >= self.view.num_signatures()
                    && index
                        < (self.view.static_account_keys().len() as u8)
                            .wrapping_sub(self.view.num_readonly_unsigned_static_accounts()))
        }
    }
}

pub fn clear_queue(
    allocator: &Allocator,
    queue: &mut VecDeque<usize>,
    offset_to_entry: &mut HashMap<usize, TransactionEntry>,
) {
    while let Some(offset) = queue.pop_front() {
        let entry = offset_to_entry.remove(&offset).expect("entry must exist");
        if let Some(loaded_addresses) = entry.loaded_addresses {
            unsafe { loaded_addresses.free(allocator) };
        }
        unsafe { entry.view.into_inner_data().free(allocator) };
    }
}