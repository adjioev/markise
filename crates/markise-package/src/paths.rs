//! Paths named by a binding or an archive: relative, inside their folder, valid everywhere.

const MAX_PATH_BYTES: usize = 1024;
const MAX_COMPONENT_BYTES: usize = 255;
const MAX_DEPTH: usize = 16;

/// Checks a `/` separated relative path and returns it without `.` components. The result is
/// empty for `.` itself. Refuses absolute paths, `..`, backslashes, colons (drive letters and
/// Windows streams), control characters, and names Windows cannot create, so that a path means
/// the same file on every platform.
pub fn clean_relative(raw: &str) -> Result<String, String> {
    if raw.len() > MAX_PATH_BYTES {
        return Err("the path is too long".to_string());
    }
    if raw.starts_with('/') {
        return Err(format!("\"{raw}\" is an absolute path"));
    }
    if raw.starts_with('~') {
        return Err(format!("\"{raw}\" points outside the folder"));
    }
    if let Some(c) = raw.chars().find(|c| matches!(c, '\\' | ':') || c.is_control()) {
        return Err(format!("\"{raw}\" holds the character {c:?}"));
    }
    let mut parts = Vec::new();
    for part in raw.split('/') {
        match part {
            "" | "." => {}
            ".." => return Err(format!("\"{raw}\" points outside the folder")),
            _ => {
                check_component(raw, part)?;
                parts.push(part);
            }
        }
    }
    if parts.len() > MAX_DEPTH {
        return Err(format!("\"{raw}\" is nested too deeply"));
    }
    Ok(parts.join("/"))
}

fn check_component(raw: &str, part: &str) -> Result<(), String> {
    if part.len() > MAX_COMPONENT_BYTES {
        return Err("a file name is too long".to_string());
    }
    if part.ends_with('.') || part.ends_with(' ') {
        return Err(format!("\"{raw}\": a name ends with a dot or a space"));
    }
    let stem = part.split('.').next().unwrap_or(part).to_ascii_uppercase();
    let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ((stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.len() == 4
            && stem.as_bytes()[3].is_ascii_digit());
    if reserved {
        return Err(format!("\"{raw}\" uses a reserved file name"));
    }
    Ok(())
}
