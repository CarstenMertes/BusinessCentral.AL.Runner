//! The tests of one area in one binary (ADR-037 item 5): fewer binaries to link. Each module is a file of reporting/.

mod reports;
mod reportext_calls;
mod reportext_currreport;
mod multi_reportext;
mod report_saveas;
mod xmlports;
mod queries;
mod query_outer_link;
