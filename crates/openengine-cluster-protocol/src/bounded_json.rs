//! Bounded decoding shared by graph-bearing trust and persistence boundaries.
use serde::Deserialize;

/// 512 JSON containers accommodate graph depth 64, payload schemas, and transport wrappers.
/// Scan before recursive serde decoding; semantic graph depth remains the verifier's authority.
const MAX_JSON_DEPTH: usize = 512;

pub fn bounded_json_from_slice<'a, T: Deserialize<'a>>(
    bytes: &'a [u8],
) -> Result<T, serde_json::Error> {
    scan_nesting(bytes)?;
    let mut decoder = serde_json::Deserializer::from_slice(bytes);
    decoder.disable_recursion_limit();
    let value = T::deserialize(&mut decoder)?;
    decoder.end()?;
    Ok(value)
}

pub fn bounded_json_from_str<'a, T: Deserialize<'a>>(
    text: &'a str,
) -> Result<T, serde_json::Error> {
    bounded_json_from_slice(text.as_bytes())
}

fn scan_nesting(bytes: &[u8]) -> Result<(), serde_json::Error> {
    let mut depth = 0_usize;
    let mut quoted = false;
    let mut escaped = false;
    for byte in bytes {
        if quoted {
            scan_string(*byte, &mut quoted, &mut escaped);
            continue;
        }
        match byte {
            b'"' => quoted = true,
            b'{' | b'[' => {
                depth += 1;
                if depth > MAX_JSON_DEPTH {
                    return Err(<serde_json::Error as serde::de::Error>::custom(
                        "JSON nesting exceeds bounded graph envelope (512)",
                    ));
                }
            }
            b'}' | b']' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    Ok(())
}

fn scan_string(byte: u8, quoted: &mut bool, escaped: &mut bool) {
    if *escaped {
        *escaped = false;
    } else if byte == b'\\' {
        *escaped = true;
    } else if byte == b'"' {
        *quoted = false;
    }
}

#[cfg(test)]
#[path = "bounded_json/tests.rs"]
mod tests;
