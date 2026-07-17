use super::*;

#[path = "regressions/binding_cases.rs"]
mod binding_cases;
#[path = "regressions/dataflow_cases.rs"]
mod dataflow_cases;
#[path = "regressions/map_cases.rs"]
mod map_cases;
#[path = "regressions/map_fixture.rs"]
mod map_fixture;
#[path = "regressions/parallel_fixture.rs"]
mod parallel_fixture;

pub(crate) use parallel_fixture::routed_writer;
