//! Final evidence gates; supported failures never become retries.
use super::*;
#[path = "gates/frames.rs"]
mod frames;
#[path = "gates/producers.rs"]
mod producers;
#[path = "gates/reads.rs"]
mod reads;
#[path = "gates/rows.rs"]
mod rows;
#[path = "gates/sql.rs"]
mod sql;
pub(super) use producers::validate_final_producers;
pub(super) use reads::validate_loaded_ready_reads;
pub(super) use rows::validate_final_rows;
pub(super) use sql::sql_only_budget_review_required;
