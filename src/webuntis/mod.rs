mod absences;
mod api;
mod client;
mod queries;
mod search;
#[cfg(test)]
mod tests;
mod timetable;

pub use client::{WebUntisClient, WebUntisError};
pub use search::search_timetable_targets;
