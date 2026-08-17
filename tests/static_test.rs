#[cfg(test)]
mod tests {
    use jawohl::{complete_json, get_closing_string_for_partial_json};
    use serde_json::from_str;

    #[test]
    fn test_complete_json() {
        let cases = vec![
            (
                r#"{"key": "value", "arr": [1, 2, {"nested_key": "nested_value""#,
                "}]}",
            ),
            (
                r#"{"foo": "bar", "nested": {"a": [1, 2, 3], "b": "text""#,
                "}}",
            ),
            (r#"["hello", "world", {"key": "value""#, "}]"),
            (
                r#"{"escaped_quote": "This is an \"escaped\" quote", "nested": [1, 2, 3, {"a": 4, "b": 5}, 6], "more_data": 1"#,
                "}",
            ),
            (
                r#"["item1", "item2", {"key1": "value1", "key2": ["sub_item1", "sub_item2", {"sub_key": "sub_value""#,
                "}]}]",
            ),
            (r#"{"name": "Bob"#, "\"}"),
        ];

        for (input, expected) in cases {
            let closing = get_closing_string_for_partial_json(input);
            assert!(closing.is_ok());
            assert_eq!(closing.unwrap(), expected);

            let complete = complete_json(input).unwrap();
            let _parsed = from_str::<serde_json::Value>(&complete);
            assert!(_parsed.is_ok());
        }
    }

    #[test]
    fn test_malformed_json() {
        let cases = vec![
            r#"{"key": "value"}}"#,
            r#"{"foo": "bar"}}"#,
            r#"["hello", "world"]]"#,
            r#"{"key": "value", "arr": [1, 2], }}]"#,
            r#"{"foo": "bar", "nested": {"a": [[1, 2, 3], "b": "text"}}"#,
        ];

        for input in cases {
            assert!(complete_json(input).is_err());
        }
    }
}
