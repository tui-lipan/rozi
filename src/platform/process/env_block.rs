//! Reading one variable out of a Windows environment block.
//!
//! Platform-independent so it is tested on every target; only the Windows backend reads another
//! process's environment.

/// The value of `key` in a UTF-16 environment block: `KEY=VALUE` entries, each ending in a NUL,
/// with an empty entry after the last.
///
/// Keys compare without regard to ASCII case, as Windows compares them, and the first entry with
/// the key wins, as `GetEnvironmentVariableW` reads it. An entry that does not end inside `block`,
/// as when the block was cut short, is not read, so a truncated value is never returned. An entry
/// whose name begins with `=`, such as the `=C:` drive-directory entries, is never a match.
pub fn find_value(block: &[u16], key: &str) -> Option<String> {
    let key: Vec<u16> = key.encode_utf16().collect();
    let mut rest = block;
    loop {
        let end = rest.iter().position(|&unit| unit == 0)?;
        let (entry, after) = rest.split_at(end);
        if entry.is_empty() {
            return None;
        }
        rest = &after[1..];
        let Some(equals) = entry
            .iter()
            .skip(1)
            .position(|&unit| unit == u16::from(b'='))
        else {
            continue;
        };
        let (name, value) = entry.split_at(equals + 1);
        if name.len() == key.len() && name.iter().zip(&key).all(|(&a, &b)| fold(a) == fold(b)) {
            return String::from_utf16(&value[1..]).ok();
        }
    }
}

fn fold(unit: u16) -> u16 {
    match u8::try_from(unit) {
        Ok(byte) => u16::from(byte.to_ascii_uppercase()),
        Err(_) => unit,
    }
}

#[cfg(test)]
mod tests {
    use super::find_value;

    fn block(entries: &[&str]) -> Vec<u16> {
        let mut units = Vec::new();
        for entry in entries {
            units.extend(entry.encode_utf16());
            units.push(0);
        }
        units.push(0);
        units
    }

    #[test]
    fn finds_the_key_ignoring_case_and_never_a_longer_or_drive_key() {
        let env = block(&[
            "=C:=C:\\work",
            "ROZI_PANE_RUNTIME_ID_X=wrong",
            "rozi_pane_runtime_id=right",
            "ROZI_PANE_RUNTIME_ID=second",
        ]);
        assert_eq!(
            find_value(&env, "ROZI_PANE_RUNTIME_ID").as_deref(),
            Some("right")
        );
        assert_eq!(find_value(&env, "ROZI_PANE").as_deref(), None);
    }

    #[test]
    fn a_missing_key_or_a_cut_block_reads_nothing() {
        let env = block(&["PATH=C:\\bin", "ROZI_PANE_RUNTIME_ID=abcdef"]);
        assert_eq!(find_value(&env, "HOME"), None);
        // Cut inside the value: its terminating NUL is gone, so neither the value nor a prefix of it
        // is returned.
        let cut = &env[..env.len() - 4];
        assert_eq!(find_value(cut, "ROZI_PANE_RUNTIME_ID"), None);
        // Text after the block's terminating empty entry is not part of it.
        let mut trailing = block(&["PATH=x"]);
        trailing.extend("ROZI_PANE_RUNTIME_ID=late\0".encode_utf16());
        assert_eq!(find_value(&trailing, "ROZI_PANE_RUNTIME_ID"), None);
    }
}
