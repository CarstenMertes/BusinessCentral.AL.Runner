//! `tables` lists in the order bcdb's culture-aware sort gives. The expected orders below
//! are the published bcdb 0.2.1's own output, captured from typeprobe.bak and the BC 28.1
//! demo backup, so a change to the approximation shows here.

use al_bcdb::query::collate_cmp;

fn sorted(names: &[&str]) -> Vec<String> {
    let mut v: Vec<String> = names.iter().map(|s| s.to_string()).collect();
    v.sort_by(|a, b| collate_cmp(a, b));
    v
}

#[test]
fn underscore_sorts_before_digits_and_case_is_secondary() {
    let expected = [
        "$probe$platform",
        "probe",
        "probe_altered",
        "probe_altered_page",
        "probe_dense",
        "probe_ghost",
        "probe_heap",
        "probe_lob",
        "probe_lob_page",
        "probe_lob_upd",
        "probe_lob2",
        "probe_notnull",
        "probe_oddnames",
        "probe_overflow",
        "probe_page",
        "probe_row",
        "probe_tracked",
        "probe_wide",
    ];
    let mut shuffled: Vec<&str> = expected.to_vec();
    shuffled.reverse();
    assert_eq!(sorted(&shuffled), expected);
}

#[test]
fn dollar_prefix_and_mixed_case() {
    let expected = [
        "$ndo$dbproperty",
        "$ndo$inplacepublishingstate",
        "$ndo$inplacepublishingworkloads",
        "$ndo$Intelligent Cloud Status Extensions",
        "$ndo$migrations",
    ];
    let mut shuffled: Vec<&str> = expected.to_vec();
    shuffled.reverse();
    assert_eq!(sorted(&shuffled), expected);
}
