mod events;
mod layout;
mod stage;

#[derive(Default)]
pub(super) struct Writers {
    pub(super) stage: stage::StageWriter,
    operations: events::LogCursor,
    errors: events::LogCursor,
}

pub(super) use events::{record_event, write_panic_report};
pub(super) use layout::{ensure_layout, read_debug_marker, root_path, state};
pub(super) use stage::{
    begin_session_marker, clear_last_stages, persist_last_stage, read_latest_stage,
};
