use std::io::Write;

use openengine_cluster_protocol::TargetRunSizesDiscovery;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use super::super::{CliOutcome, NativeV2CliBackend, NativeV2CliError};

pub(super) async fn execute(
    backend: &impl NativeV2CliBackend,
    name: &str,
    json: bool,
    output: &mut impl Write,
) -> Result<CliOutcome, NativeV2CliError> {
    let catalog = backend.target_sizes(name).await?;
    if json {
        super::write_json(output, &catalog)?;
    } else {
        let width = std::env::var("COLUMNS")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(80)
            .clamp(40, 240);
        write_table(output, &catalog, width)?;
    }
    Ok(CliOutcome::Completed)
}

fn write_table(
    output: &mut impl Write,
    catalog: &TargetRunSizesDiscovery,
    width: usize,
) -> Result<(), NativeV2CliError> {
    if catalog.sizes.is_empty() {
        writeln!(output, "No sizes are currently offered by this target.")?;
        return Ok(());
    }
    let rows = catalog
        .sizes
        .iter()
        .map(|size| {
            let id = size.id.as_str();
            let display = if id.chars().any(char::is_whitespace) {
                serde_json::to_string(id)?
            } else {
                id.to_owned()
            };
            Ok((display, &size.description))
        })
        .collect::<Result<Vec<_>, serde_json::Error>>()?;
    let id_width = rows
        .iter()
        .map(|(id, _)| id.width())
        .max()
        .unwrap_or(0)
        .max(10)
        .min(width / 3);
    let description_width = width - id_width - 2;
    writeln!(output, "{:<id_width$}  Description", "Identifier")?;
    writeln!(
        output,
        "{}  {}",
        "-".repeat(id_width),
        "-".repeat(description_width)
    )?;
    for (id, description) in rows {
        let ids = wrap_identifier(&id, id_width);
        let descriptions = wrap(description, description_width);
        for index in 0..ids.len().max(descriptions.len()) {
            let id = ids.get(index).map(String::as_str).unwrap_or("");
            let description = descriptions.get(index).map(String::as_str).unwrap_or("");
            let padding = id_width.saturating_sub(id.width());
            writeln!(output, "{id}{}  {description}", " ".repeat(padding))?;
        }
    }
    Ok(())
}

fn wrap_identifier(value: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for character in value.chars() {
        if !line.is_empty() && line.width() + character.width().unwrap_or(0) > width {
            lines.push(std::mem::take(&mut line));
        }
        line.push(character);
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

fn wrap(value: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in value.split_whitespace() {
        let candidate = if line.is_empty() {
            word.to_owned()
        } else {
            format!("{line} {word}")
        };
        if candidate.width() <= width {
            line = candidate;
            continue;
        }
        if !line.is_empty() {
            lines.push(std::mem::take(&mut line));
        }
        let mut fragments = wrap_identifier(word, width);
        line = fragments.pop().unwrap_or_default();
        lines.extend(fragments);
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

#[cfg(test)]
#[path = "sizes/tests.rs"]
mod tests;
