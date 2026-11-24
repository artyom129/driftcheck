//! driftcheck compares two exports of the same data record by record and says
//! whether the migration can be accepted.

pub mod config;
pub mod data;
pub mod fuzzy;
pub mod gen;
pub mod infer;
pub mod patch;
pub mod reconcile;
pub mod report;
pub mod value;
