//! GENERATED-block markers. A generated block is delimited by HTML comments,
//! invisible in rendered Markdown:
//!
//!   <!-- BEGIN GENERATED:<id> -->
//!   ...body...
//!   <!-- END GENERATED:<id> -->
//!
//! plumbline owns the body between the markers and rewrites it from a real run
//! of the crate's binary; the marker lines themselves are preserved.

pub fn begin_marker(id: &str) -> String {
    format!("<!-- BEGIN GENERATED:{id} -->")
}

pub fn end_marker(id: &str) -> String {
    format!("<!-- END GENERATED:{id} -->")
}

/// Return the body between the BEGIN/END markers for `id`, excluding the marker
/// lines and the single newline bounding the body on each side. None if the
/// block is absent or malformed.
pub fn extract_generated<'a>(text: &'a str, id: &str) -> Option<&'a str> {
    let begin = begin_marker(id);
    let end = end_marker(id);
    let after_begin = text.find(&begin)? + begin.len();
    let body_start = after_begin + text[after_begin..].starts_with('\n').then_some(1)?;
    let estart = text[body_start..].find(&end)? + body_start;
    let body_end = text[..estart].strip_suffix('\n')?.len();
    Some(&text[body_start..body_end])
}

/// Replace the body between the markers for `id` with `body`, preserving the
/// marker lines. Errors if either marker is absent.
pub fn replace_generated(text: &str, id: &str, body: &str) -> Result<String, String> {
    let begin = begin_marker(id);
    let end = end_marker(id);
    let after_begin = text
        .find(&begin)
        .ok_or_else(|| format!("BEGIN marker for `{id}` not found"))?
        + begin.len();
    let estart = text[after_begin..]
        .find(&end)
        .ok_or_else(|| format!("END marker for `{id}` not found"))?
        + after_begin;
    Ok(format!("{}\n{body}\n{}", &text[..after_begin], &text[estart..]))
}

/// Every GENERATED-block id present in `text`, in order of appearance.
pub fn generated_ids(text: &str) -> Vec<String> {
    const NEEDLE: &str = "BEGIN GENERATED:";
    let mut ids = Vec::new();
    let mut rest = text;
    while let Some(i) = rest.find(NEEDLE) {
        let after = &rest[i + NEEDLE.len()..];
        let id = after.split([' ', '\n', '\r', '\t']).next().unwrap_or("");
        if !id.is_empty() {
            ids.push(id.to_string());
        }
        rest = after;
    }
    ids
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str =
        "pre\n<!-- BEGIN GENERATED:readme-example -->\nold body\nline2\n<!-- END GENERATED:readme-example -->\npost\n";

    #[test]
    fn extract_returns_body_without_marker_lines() {
        assert_eq!(extract_generated(SAMPLE, "readme-example"), Some("old body\nline2"));
    }

    #[test]
    fn extract_absent_block_is_none() {
        assert_eq!(extract_generated("no markers here", "readme-example"), None);
    }

    #[test]
    fn replace_preserves_surroundings_and_round_trips() {
        let updated = replace_generated(SAMPLE, "readme-example", "new body\nnew line2").unwrap();
        assert!(updated.starts_with("pre\n"));
        assert!(updated.ends_with("post\n"));
        assert_eq!(
            extract_generated(&updated, "readme-example"),
            Some("new body\nnew line2")
        );
    }

    #[test]
    fn replace_errors_when_marker_absent() {
        assert!(replace_generated("no markers", "readme-example", "x").is_err());
    }

    #[test]
    fn generated_ids_lists_hyphenated_ids_in_order() {
        let text = "<!-- BEGIN GENERATED:readme-example -->\nx\n<!-- END GENERATED:readme-example -->\n\
                    <!-- BEGIN GENERATED:exit-codes-table -->\ny\n<!-- END GENERATED:exit-codes-table -->\n";
        assert_eq!(
            generated_ids(text),
            vec!["readme-example".to_string(), "exit-codes-table".to_string()]
        );
    }
}
