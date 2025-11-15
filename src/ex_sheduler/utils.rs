use std::ptr::NonNull;

use agave_scheduler_bindings::{
    SharablePubkeys, TransactionResponseRegion,
    worker_message_types::{self, CheckResponse, ExecutionResponse},
};
use rts_alloc::Allocator;
use solana_pubkey::Pubkey;

pub struct PubkeysPtr {
    ptr: NonNull<Pubkey>,
    count: usize,
}

impl PubkeysPtr {
    pub unsafe fn from_sharable_pubkeys(
        sharable_pubkeys: &SharablePubkeys,
        allocator: &Allocator,
    ) -> Self {
        let ptr = allocator.ptr_from_offset(sharable_pubkeys.offset).cast();
        Self {
            ptr,
            count: sharable_pubkeys.num_pubkeys as usize,
        }
    }

    pub fn as_slice(&self) -> &[Pubkey] {
        unsafe { core::slice::from_raw_parts(self.ptr.as_ptr(), self.count) }
    }

    pub unsafe fn free(self, allocator: &Allocator) {
        unsafe { allocator.free(self.ptr.cast()) };
    }
}

pub struct ExecutionResponsesPtr<'a> {
    ptr: NonNull<ExecutionResponse>,
    count: usize,
    allocator: &'a Allocator,
}

impl<'a> ExecutionResponsesPtr<'a> {
    pub unsafe fn from_transaction_response_region(
        transaction_response_region: &TransactionResponseRegion,
        allocator: &'a Allocator,
    ) -> Self {
        debug_assert!(
            transaction_response_region.tag == worker_message_types::EXECUTION_RESPONSE
        );

        Self {
            ptr: allocator
                .ptr_from_offset(transaction_response_region.transaction_responses_offset)
                .cast(),
            count: transaction_response_region.num_transaction_responses as usize,
            allocator,
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = &ExecutionResponse> {
        unsafe { core::slice::from_raw_parts(self.ptr.as_ptr(), self.count) }.iter()
    }

    pub fn free_wrapper(self) {
        unsafe { self.allocator.free(self.ptr.cast()) }
    }
}

pub struct CheckResponsesPtr<'a> {
    ptr: NonNull<CheckResponse>,
    count: usize,
    allocator: &'a Allocator,
}

impl<'a> CheckResponsesPtr<'a> {
    pub unsafe fn from_transaction_response_region(
        transaction_response_region: &TransactionResponseRegion,
        allocator: &'a Allocator,
    ) -> Self {
        debug_assert!(transaction_response_region.tag == worker_message_types::CHECK_RESPONSE);

        Self {
            ptr: allocator
                .ptr_from_offset(transaction_response_region.transaction_responses_offset)
                .cast(),
            count: transaction_response_region.num_transaction_responses as usize,
            allocator,
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = &CheckResponse> {
        unsafe { core::slice::from_raw_parts(self.ptr.as_ptr(), self.count) }.iter()
    }

    pub fn free_wrapper(self) {
        unsafe { self.allocator.free(self.ptr.cast()) }
    }
}