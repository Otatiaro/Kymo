use super::*;

fn words(text: &str) -> Words {
    let mut words = Words::default();
    words.set(text);
    words
}

#[test]
fn a_type_stays_with_one_plain_word() {
    let words = words("Locked  started");
    assert!(words.active());
    assert!(words.keeps("kymo::opsy::mutex_locked"));
    assert!(words.keeps("kymo::opsy::task_started"));
    assert!(!words.keeps("kymo::opsy::task_ready"));
}

#[test]
fn a_type_stays_with_every_word_after_a_plus_and_none_after_a_minus() {
    let filter = words("Opsy +mutex -wait");
    assert!(filter.keeps("kymo::opsy::mutex_locked"));
    assert!(!filter.keeps("kymo::opsy::mutex_wait"));
    assert!(!filter.keeps("kymo::opsy::task_started"));
    assert!(!filter.keeps("app::mutex_locked"));
    let filter = words("+mutex +locked");
    assert!(filter.keeps("kymo::opsy::mutex_locked"));
    assert!(!filter.keeps("kymo::opsy::mutex_taken"));
}

#[test]
fn a_sign_alone_or_no_word_leaves_out_nothing() {
    for text in ["", "  ", "-", "+", "- +"] {
        let words = words(text);
        assert!(!words.active(), "{text:?}");
        assert!(words.keeps("kymo::opsy::task_started"), "{text:?}");
    }
    assert!(words("-wait").keeps("kymo::opsy::task_started"));
    assert!(words("+task").keeps("kymo::opsy::task_started"));
}
