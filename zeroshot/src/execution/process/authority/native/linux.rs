use std::fs::File;
use std::io::{self, Read};

use super::ProcessIdentity;

const MAX_STAT_BYTES: u64 = 4096;

pub(crate) fn process_identity(pid: u32) -> io::Result<ProcessIdentity> {
    let mut stat = Vec::new();
    File::open(format!("/proc/{pid}/stat"))?
        .take(MAX_STAT_BYTES + 1)
        .read_to_end(&mut stat)?;
    if stat.len() as u64 > MAX_STAT_BYTES {
        return Err(invalid_stat());
    }
    parse_stat(pid, &stat)
}

fn parse_stat(pid: u32, stat: &[u8]) -> io::Result<ProcessIdentity> {
    let open = stat
        .iter()
        .position(|byte| *byte == b'(')
        .ok_or_else(invalid_stat)?;
    let close = stat
        .iter()
        .rposition(|byte| *byte == b')')
        .ok_or_else(invalid_stat)?;
    if open >= close {
        return Err(invalid_stat());
    }
    let prefix = std::str::from_utf8(&stat[..open]).map_err(|_| invalid_stat())?;
    let actual: u32 = prefix.trim().parse().map_err(|_| invalid_stat())?;
    if actual != pid {
        return Err(invalid_stat());
    }
    // comm may contain spaces, parentheses or non-UTF-8 bytes. Numeric fields follow its final ')'.
    let suffix = std::str::from_utf8(&stat[close + 1..]).map_err(|_| invalid_stat())?;
    let mut fields = suffix.split_ascii_whitespace();
    if fields.next().is_none_or(|state| state.len() != 1) {
        return Err(invalid_stat());
    }
    let parent = stat_number(fields.next())?;
    let group = stat_number(fields.next())?;
    // After state (field 3), parent (4) and group (5), starttime is field 22.
    let born = stat_number(fields.nth(16))?;
    Ok(ProcessIdentity {
        pid,
        parent,
        group,
        born,
    })
}

fn stat_number<T: std::str::FromStr>(field: Option<&str>) -> io::Result<T> {
    field
        .ok_or_else(invalid_stat)?
        .parse()
        .map_err(|_| invalid_stat())
}

fn invalid_stat() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "invalid native process stat record",
    )
}

#[cfg(test)]
mod tests;
