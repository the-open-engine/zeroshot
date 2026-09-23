use super::*;

#[test]
fn basic_credential_encoding_is_canonical() {
    assert_eq!(
        encode_basic_credential("token"),
        "eC1hY2Nlc3MtdG9rZW46dG9rZW4="
    );
}
