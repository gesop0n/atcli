//! Compile a C++ solution and judge it against local test cases.
//!
//! The crate deliberately prints nothing: [`judge`] reports each case as a
//! [`CaseResult`] and leaves presentation to the caller.

mod build;
mod diagnose;
mod run;

pub use build::{BuildOutcome, BuildRequest, BuildSettings, CompileError, build};
pub use diagnose::{Diagnostics, SourceLocation, crash_source_location, find_source_location};
pub use run::{
    CaseResult, JudgeSettings, Outcome, Summary, TestCase, discover_cases, judge,
    normalized_output, outputs_equal,
};
