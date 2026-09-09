#![allow(clippy::expect_used)]

mod algebra_properties;
mod assertions;
mod commutation;
mod confluence;
mod create;
mod deferred;
mod evidence;
mod fixtures;
mod independence;
mod merge_evidence_precedence;
mod merge_evidence_report;
mod merge_evidence_report_privacy;
mod oracle;
mod ordering;
mod rename_destination_conflict;
mod replacement_evidence;
mod same_node;
mod support;

use assertions::*;
use fixtures::*;
use oracle::*;
use support::*;
