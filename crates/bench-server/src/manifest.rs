//! `bench/cells.json`: what each cell serves, for the harness and for clients that verify the body checksum.

use crate::cells::{Cell, cells};
use crate::request::find_head_end;

/// The cell manifest as JSON (one object per line), the content of `bench/cells.json`.
#[must_use]
pub fn cells_json() -> String {
    let rows: Vec<String> = cells().iter().map(row).collect();
    format!("[\n{}\n]\n", rows.join(",\n"))
}

fn row(cell: &Cell) -> String {
    format!(
        r#"  {{ "path": "{}", "responseBytes": {}, "headerCount": {}, "bodyFnv1a64": "{:#018x}" }}"#,
        cell.path,
        cell.response.len(),
        header_count(&cell.response),
        cell.body_fnv,
    )
}

/// Field lines in a response head: the line breaks between the status line and the blank line.
fn header_count(response: &[u8]) -> usize {
    let head = &response[..find_head_end(response, 0).unwrap_or(0)];
    head.windows(2).filter(|w| w == b"\r\n").count()
}

#[cfg(test)]
mod tests {
    use super::header_count;

    #[test]
    fn counts_field_lines() {
        assert_eq!(
            header_count(b"HTTP/1.1 200 OK\r\na: 1\r\nb: 2\r\n\r\nbody\r\n"),
            2
        );
        assert_eq!(header_count(b"HTTP/1.1 200 OK\r\n\r\n"), 0);
    }
}
