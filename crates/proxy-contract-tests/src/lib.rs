/// Returns the pinned Katamari commit used as the starting behavior contract.
#[must_use]
pub fn source_behavior_commit() -> &'static str {
    "ab5e90f6a2ff05a063663ce478146bf0b6829429"
}

#[cfg(test)]
mod tests {
    use super::source_behavior_commit;

    #[test]
    fn pins_the_behavior_source_commit() {
        assert_eq!(
            source_behavior_commit(),
            "ab5e90f6a2ff05a063663ce478146bf0b6829429"
        );
    }

    #[test]
    fn behavior_contract_doc_mentions_next_characterization_tests() {
        let doc = include_str!("../../../docs/behavior-contract.md");

        assert!(doc.contains("First Characterization Tests To Write Next"));
        assert!(doc.contains("Worker registration handshake"));
        assert!(doc.contains("Streaming pass-through"));
    }
}
