/// Prepare transcript text before measuring or wrapping it. Ratatui passes
/// literal tabs through to the backend, where they move the terminal cursor
/// outside its cell layout. Expand them relative to the source line, so tab
/// stops stay the same when the viewport wraps that line or adds panel padding.
pub(super) fn sanitize(text: &str) -> String {
    let text = crate::tools::sanitize_terminal_output(text);
    if !text.contains('\t') {
        return text;
    }
    let mut out = String::with_capacity(text.len());
    for (index, line) in text.split('\n').enumerate() {
        if index > 0 {
            out.push('\n');
        }
        let mut column = 0;
        for (index, part) in line.split('\t').enumerate() {
            if index > 0 {
                let spaces = 8 - column % 8;
                out.extend(std::iter::repeat_n(' ', spaces));
                column += spaces;
            }
            out.push_str(part);
            column += unicode_width::UnicodeWidthStr::width(part);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tabs_use_display_columns_and_restart_on_each_source_line() {
        assert_eq!(sanitize("a\tb\t0"), "a       b       0");
        assert_eq!(sanitize("12345678\tx\n\ty"), "12345678        x\n        y");
        assert_eq!(sanitize("项目\te\u{301}\tx"), "项目    e\u{301}       x");
        assert_eq!(sanitize("\t\tx\t"), "                x       ");
    }

    #[test]
    fn tab_stops_measure_visible_text_after_terminal_controls_are_removed() {
        assert_eq!(
            sanitize("old\r\x1b[31mnew\x1b[0m\tvalue\r\n"),
            "new     value\n"
        );
    }
}
