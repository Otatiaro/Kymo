use super::*;

fn words(text: &str) -> Words {
    let mut words = Words::default();
    words.set(text);
    words
}

#[test]
fn a_type_stays_with_every_word_and_none_after_a_minus() {
    let words = words("Opsy  mutex -wait");
    assert!(words.active());
    assert!(words.keeps("kymo::opsy::mutex_locked"));
    assert!(!words.keeps("kymo::opsy::mutex_wait"));
    assert!(!words.keeps("kymo::opsy::task_started"));
    assert!(!words.keeps("app::mutex_locked"));
}

#[test]
fn a_minus_alone_or_no_word_leaves_out_nothing() {
    for text in ["", "  ", "-"] {
        let words = words(text);
        assert!(!words.active(), "{text:?}");
        assert!(words.keeps("kymo::opsy::task_started"), "{text:?}");
    }
    assert!(words("-wait").keeps("kymo::opsy::task_started"));
}
