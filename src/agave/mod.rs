mod pack_to_worker;
mod progress_tracker;
pub mod server;
mod tpu_to_pack;
mod worker;
mod worker_to_pack;

pub const SLEEP_PROGRESS_TRACKER: u64 = 60;
pub const SLEEP_TPU_TO_PACK: u64 = 40;
pub const SLEEP_PACK_TO_WORKER: u64 = 0;
pub const SLEEP_WORKER_TO_PACK: u64 = 0;
pub const SLEEP_WORKER: u64 = 0;
