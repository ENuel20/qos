use agave_scheduler_bindings::ProgressMessage;
use shaq::Consumer;

pub fn handle_progress_message(
    progress_tracker: &mut Consumer<ProgressMessage>,
) -> Option<(bool, u64)> {
    progress_tracker.sync();

    let mut new_is_leader = None;
    let mut slots_until_leader = u64::MAX;
    let message_count = progress_tracker.len();
    for _ in 0..(message_count.saturating_sub(1)) {
        let _ = progress_tracker.try_read();
    }
    if let Some(message) = progress_tracker.try_read() {
        new_is_leader = Some(message.leader_state == agave_scheduler_bindings::IS_LEADER);
        slots_until_leader = message
            .next_leader_slot
            .saturating_sub(message.current_slot);
    }

    progress_tracker.finalize();
    new_is_leader.map(|is_leader| (is_leader, slots_until_leader))
}