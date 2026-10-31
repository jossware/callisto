//! Obsidian-safe markdown from bead text, after ganymede's escaping.

use std::collections::{HashMap, VecDeque};

/// Text for an unquoted entry.
pub fn markdown(text: &str) -> String {
    let text = normalize_newlines(text);
    segments(&text)
        .into_iter()
        .map(|segment| match segment {
            Segment::Prose(lines) => prose(&lines),
            Segment::Fence(fence) => fence,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn fence_for(content: &str) -> String {
    "`".repeat(longest_run(content, '`').max(2) + 1)
}

/// A tool or agent name inside a one-line bold label. Every CommonMark and Obsidian marker
/// is backslash-escaped, since a name has no formatting worth keeping.
pub fn label(text: &str) -> String {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut out = String::with_capacity(collapsed.len());
    for c in collapsed.chars() {
        if "\\`*_[]<>#|~=%^$!".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

fn longest_run(s: &str, ch: char) -> usize {
    s.split(|c| c != ch).map(str::len).max().unwrap_or(0)
}

/// Rust's `trim` also strips Unicode spaces such as NBSP, which CommonMark
/// treats as text.
const MD_SPACE: [char; 2] = [' ', '\t'];

/// CommonMark ends a line at `\r` too; a scanner splitting only on `\n` would
/// match code spans across lines Obsidian has already split.
fn normalize_newlines(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

fn is_blank(s: &str) -> bool {
    s.trim_matches(MD_SPACE).is_empty()
}

fn run_len(s: &str, ch: char) -> usize {
    s.len() - s.trim_start_matches(ch).len()
}

enum Segment<'a> {
    Prose(Vec<&'a str>),
    Fence(String),
}

struct Opener {
    indent: usize,
    width: usize,
    ch: char,
    len: usize,
}

fn segments(text: &str) -> Vec<Segment<'_>> {
    let mut out = Vec::new();
    let mut prose = Vec::new();
    let all: Vec<&str> = text.split('\n').collect();
    let start = leading_code(&all, &mut out);
    let mut lines = all[start..].iter().copied();
    while let Some(line) = lines.next() {
        let Some(open) = opener(line) else {
            prose.push(line);
            continue;
        };
        if !prose.is_empty() {
            out.push(Segment::Prose(std::mem::take(&mut prose)));
        }
        let mut body = Vec::new();
        let mut closed = false;
        for line in lines.by_ref() {
            if closes(line, &open) {
                closed = true;
                break;
            }
            body.push(dedent(line, open.indent));
        }
        // A terminal newline leaves an empty last line that is not content.
        if !closed && body.last() == Some(&"") {
            body.pop();
        }
        out.push(Segment::Fence(fence(line, &open, &body)));
    }
    if !prose.is_empty() {
        out.push(Segment::Prose(prose));
    }
    out
}

/// With nothing before it, a run indented four columns is an indented code
/// block, never a list continuation. As a fence its content stays literal and
/// it ends a list the note has open. A fence opener wins at any indent, as it
/// does everywhere else here.
fn leading_code<'a>(lines: &[&'a str], out: &mut Vec<Segment<'a>>) -> usize {
    let Some(start) = lines.iter().position(|l| !is_blank(l)) else {
        return 0;
    };
    let code = |l: &str| indent_width(l) >= 4 && opener(l).is_none();
    if !code(lines[start]) {
        return 0;
    }
    let run = lines[start..]
        .iter()
        .position(|l| !is_blank(l) && !code(l))
        .map_or(lines.len(), |i| start + i);
    let end = lines[..run]
        .iter()
        .rposition(|l| !is_blank(l))
        .map_or(start, |i| i + 1);
    if start > 0 {
        out.push(Segment::Prose(lines[..start].to_vec()));
    }
    let content = lines[start..end]
        .iter()
        .map(|l| strip_columns(l, 4))
        .collect::<Vec<_>>()
        .join("\n");
    let fence = fence_for(&content);
    out.push(Segment::Fence(format!("{fence}\n{content}\n{fence}")));
    end
}

fn strip_columns(line: &str, cols: usize) -> &str {
    let mut width = 0;
    for (i, c) in line.char_indices() {
        if width >= cols {
            return &line[i..];
        }
        match c {
            ' ' => width += 1,
            '\t' => width += 4 - width % 4,
            _ => return &line[i..],
        }
    }
    ""
}

fn opener(line: &str) -> Option<Opener> {
    let body = line.trim_start_matches([' ', '\t']);
    let ch = body.chars().next().filter(|c| matches!(c, '`' | '~'))?;
    let len = run_len(body, ch);
    // A backtick in the info string makes the line an inline code span.
    if len < 3 || (ch == '`' && body[len..].contains('`')) {
        return None;
    }
    Some(Opener {
        indent: line.len() - body.len(),
        width: indent_width(line),
        ch,
        len,
    })
}

/// The delimiter is rebuilt longer than any run in the content, so no content
/// line can close it, however dedenting shifts it.
fn fence(opener: &str, open: &Opener, body: &[&str]) -> String {
    let info = &opener.trim_start_matches([' ', '\t'])[open.len..];
    let longest = body
        .iter()
        .map(|l| longest_run(l, open.ch))
        .max()
        .unwrap_or(0);
    let run = open.ch.to_string().repeat(open.len.max(longest + 1));
    let mut out = format!("{run}{info}");
    for line in body {
        out.push('\n');
        out.push_str(line);
    }
    out.push('\n');
    out.push_str(&run);
    out
}

/// CommonMark reads a run indented 4+ columns past the margin as content.
fn closes(line: &str, open: &Opener) -> bool {
    let body = line.trim_start_matches([' ', '\t']);
    let len = run_len(body, open.ch);
    indent_width(line) <= open.width + 3 && len >= open.len && is_blank(&body[len..])
}

fn indent_width(line: &str) -> usize {
    let mut width = 0;
    for c in line.chars() {
        match c {
            ' ' => width += 1,
            '\t' => width += 4 - width % 4,
            _ => break,
        }
    }
    width
}

/// Leading whitespace would stop a fence from being a fence at column 0.
fn dedent(line: &str, indent: usize) -> &str {
    let ws = line.len() - line.trim_start_matches([' ', '\t']).len();
    &line[ws.min(indent)..]
}

/// Code spans are matched within a line. A span crossing lines could run past
/// a block CommonMark ends first, such as an HTML block, and copy it raw.
/// Indented code is copied verbatim, since CommonMark shows it literally.
fn prose(lines: &[&str]) -> String {
    let (blocks, fresh) = blocks(lines);
    let mut out = String::new();
    for (i, line) in lines.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        if fresh[i] {
            out.push('\n');
        }
        if blocks[i] == Block::Code {
            out.push_str(line);
        } else if let Block::Underline(at) = blocks[i] {
            out.push_str(&line[..at]);
            out.push_str(&underline(&line[at..]));
        } else if let Block::Heading(at, level) = blocks[i] {
            out.push_str(&line[..at]);
            out.push_str(&"#".repeat((level + 2).min(6)));
            inline(&line[at + level..], &mut out);
        } else {
            let n = quote_marker(line);
            out.push_str(&line[..n]);
            // Obsidian reads `> [!type]` as a callout, which bead text must not forge.
            // Obsidian may trim Unicode spaces that CommonMark keeps as text.
            let rest = &line[n..];
            let gap = rest.len() - rest.trim_start_matches(char::is_whitespace).len();
            if n > 0 && rest[gap..].starts_with("[!") {
                out.push_str(&rest[..gap]);
                out.push('\\');
                inline(&rest[gap..], &mut out);
            } else {
                inline(rest, &mut out);
            }
        }
    }
    out
}

/// The byte length of a line's leading blockquote markers, or 0 without one.
fn quote_marker(line: &str) -> usize {
    let body = line.trim_start_matches([' ', '\t']);
    if !body.starts_with('>') {
        return 0;
    }
    let marker = body.trim_start_matches(['>', ' ', '\t']);
    line.len() - marker.len()
}

#[derive(Clone, Copy, PartialEq)]
enum Block {
    Text,
    Code,
    /// The byte offset of the `#` run, and its length.
    Heading(usize, usize),
    /// A setext underline, by the byte offset of its run.
    Underline(usize),
}

/// Bead text sits under the note's `##` sections, so its own headings
/// move two levels down; an H1 would also break the vault's no-H1 rule.
fn heading_level(content: &str) -> Option<usize> {
    let level = run_len(content, '#');
    let rest = &content[level..];
    let ends = rest.is_empty() || rest.starts_with([' ', '\t']);
    ((1..=6).contains(&level) && ends).then_some(level)
}

/// How CommonMark reads each line: indented code, an ATX heading, a setext
/// underline, or text. Escaping keeps list markers, so the source's lists are
/// the note's: four columns past a list item's content column is code, and
/// anything less is the item's continuation, where a heading's or underline's
/// indent counts from that column.
///
/// The second vector marks a non-quote line, or one indented into code, right
/// after a text quote line. The escaper does not read inside quotes, so it
/// cannot tell where CommonMark ends the quote's paragraph. Reading such a
/// line as after a blank makes both agree, and `prose` emits that blank.
fn blocks(lines: &[&str]) -> (Vec<Block>, Vec<bool>) {
    let mut blocks = vec![Block::Text; lines.len()];
    let mut fresh = vec![false; lines.len()];
    let mut after_quote = false;
    let mut items: Vec<Item> = Vec::new();
    let mut paragraph = false;
    for (i, line) in lines.iter().enumerate() {
        if is_blank(line) {
            paragraph = false;
            after_quote = false;
            continue;
        }
        let width = indent_width(line);
        let container = items
            .iter()
            .rev()
            .map(|item| item.content)
            .find(|&content| content <= width)
            .unwrap_or(0);
        // Indented code is no quote, whatever its first character.
        if after_quote && (quote_marker(line) == 0 || width >= container + 4) {
            fresh[i] = true;
            paragraph = false;
        }
        after_quote = false;
        let content = line.trim_start_matches([' ', '\t']);
        if width >= container + 4 {
            // Indented code cannot interrupt a paragraph.
            if !paragraph {
                blocks[i] = Block::Code;
                items.retain(|item| item.content <= width);
            }
            continue;
        }
        if paragraph && is_underline(content) {
            blocks[i] = Block::Underline(line.len() - content.len());
            // Only the rule ends the paragraph; an escaped run continues it.
            if is_thematic_break(content) {
                items.retain(|item| item.content <= width);
                paragraph = false;
            }
            continue;
        }
        if is_thematic_break(content) {
            items.retain(|item| item.content <= width);
            paragraph = false;
            continue;
        }
        let open = list_item(line, width).filter(|item| {
            // Only a non-empty bullet or a list from 1 may interrupt a paragraph,
            // and only one in the item the line belongs to.
            let interrupts = paragraph && items.last().is_none_or(|open| open.content <= width);
            !interrupts || (!item.empty && item.number.is_none_or(|n| n == 1))
        });
        let heading_at =
            |content: &str| Some((line.len() - content.len(), heading_level(content)?));
        let mut heading = if open.is_none() {
            heading_at(content)
        } else {
            None
        };
        // A heading or a quote is never a paragraph's lazy continuation, so
        // it closes the items it is not indented into.
        if open.is_some() || !paragraph || heading.is_some() || quote_marker(line) > 0 {
            items.retain(|item| item.content <= width);
        }
        // Text left on the line, unless it is a heading, a rule or code.
        let mut text = heading.is_none().then_some(content);
        let mut next = open;
        while let Some(item) = next {
            let rest = item.rest.filter(|rest| !is_thematic_break(rest));
            next = rest.and_then(|rest| list_item(rest, item.content));
            if next.is_none() {
                heading = rest.and_then(heading_at);
                text = rest.filter(|_| heading.is_none());
            }
            items.push(item);
        }
        paragraph = text.is_some();
        after_quote = text.is_some() && quote_marker(line) > 0;
        if let Some((at, level)) = heading {
            blocks[i] = Block::Heading(at, level);
        }
    }
    (blocks, fresh)
}

struct Item<'a> {
    number: Option<u32>,
    content: usize,
    empty: bool,
    /// Content that may open a nested item on the same line.
    rest: Option<&'a str>,
}

/// The list item `line` opens, if it opens one.
fn list_item(line: &str, width: usize) -> Option<Item<'_>> {
    let body = line.trim_start_matches([' ', '\t']);
    let (number, marker) = if body.starts_with(['-', '*', '+']) {
        (None, 1)
    } else {
        let digits = body.bytes().take_while(u8::is_ascii_digit).count();
        let kind = body[digits..].chars().next()?;
        if !(1..=9).contains(&digits) || !matches!(kind, '.' | ')') {
            return None;
        }
        (body[..digits].parse().ok(), digits + 1)
    };
    let rest = &body[marker..];
    let start = width + marker;
    let mut col = start;
    for c in rest.chars() {
        match c {
            ' ' => col += 1,
            '\t' => col += 4 - col % 4,
            _ => break,
        }
    }
    let gap = col - start;
    let empty = is_blank(rest);
    if gap == 0 && !empty {
        return None;
    }
    // Five columns or more after the marker is code, which starts one in.
    let nested = !empty && gap <= 4;
    Some(Item {
        number,
        content: if nested { col } else { start + 1 },
        empty,
        rest: nested.then(|| rest.trim_start_matches([' ', '\t'])),
    })
}

fn is_underline(content: &str) -> bool {
    let run = content.trim_end_matches(MD_SPACE);
    !run.is_empty() && (run.bytes().all(|b| b == b'-') || run.bytes().all(|b| b == b'='))
}

/// Three or more of one of `-`, `*` or `_`, with spaces or tabs between.
fn is_thematic_break(content: &str) -> bool {
    let mut marks = content.chars().filter(|c| !MD_SPACE.contains(c));
    let Some(mark) = marks.next().filter(|c| matches!(c, '-' | '*' | '_')) else {
        return false;
    };
    marks.clone().all(|c| c == mark) && marks.count() >= 2
}

/// `***` rather than `<hr>`, which would open an HTML block that copies
/// every line up to the next blank one raw.
fn underline(content: &str) -> String {
    let run = content.trim_end_matches(MD_SPACE);
    if is_thematic_break(content) {
        "***".to_string()
    } else {
        format!("\\{run}")
    }
}

fn inline(text: &str, out: &mut String) {
    let mut runs = backtick_runs(text);
    let mut i = 0;
    let mut after_space = true;
    let mut backslashes = 0;
    while let Some(c) = text[i..].chars().next() {
        let rest = &text[i..];
        let escaped = backslashes % 2 == 1;
        match c {
            // Backslashes are literal inside a span, so only an opener can be escaped.
            '`' if !escaped => {
                let n = run_len(rest, '`');
                let len = closing_run(&mut runs, n, i + n).map_or(n, |end| end + n - i);
                out.push_str(&rest[..len]);
                i += len;
                after_space = false;
                backslashes = 0;
                continue;
            }
            '<' | '>' => {
                // An odd backslash run would otherwise escape the entity's `&`.
                if escaped {
                    out.push('\\');
                }
                out.push_str(if c == '<' { "&lt;" } else { "&gt;" });
            }
            '#' if after_space && rest[1..].chars().next().is_some_and(is_tag_char) => {
                out.push_str("\\#");
            }
            // Two `%` open an Obsidian comment that hides text up to the next pair.
            '%' if text[..i].ends_with('%') || rest[1..].starts_with('%') => {
                if !escaped {
                    out.push('\\');
                }
                out.push('%');
            }
            _ => out.push(c),
        }
        after_space = c.is_whitespace();
        backslashes = if c == '\\' { backslashes + 1 } else { 0 };
        i += c.len_utf8();
    }
}

/// The start of every maximal backtick run, keyed by run length, in order.
fn backtick_runs(text: &str) -> HashMap<usize, VecDeque<usize>> {
    let mut runs = HashMap::<usize, VecDeque<usize>>::new();
    let mut i = 0;
    while let Some(offset) = text[i..].find('`') {
        let start = i + offset;
        let len = run_len(&text[start..], '`');
        runs.entry(len).or_default().push_back(start);
        i = start + len;
    }
    runs
}

/// CommonMark closes a span only on a run of equal length. Openers only move
/// forward, so runs before `from` are dropped for good, keeping the scan linear.
fn closing_run(runs: &mut HashMap<usize, VecDeque<usize>>, n: usize, from: usize) -> Option<usize> {
    let starts = runs.get_mut(&n)?;
    while starts.front().is_some_and(|&start| start < from) {
        starts.pop_front();
    }
    starts.front().copied()
}

fn is_tag_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | '-' | '/')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fence_for_outgrows_backtick_runs() {
        for (content, want) in [
            ("a\nb", "```"),
            ("", "```"),
            ("# Doc\n\n```markdown\n## Heading\n```\n\nmore text", "````"),
            ("before\n````\nnested\n````\nafter", "`````"),
        ] {
            assert_eq!(fence_for(content), want, "content: {content:?}");
        }
    }

    #[test]
    fn leading_indented_code_becomes_a_fence() {
        for (text, want) in [
            ("    if x < y", "```\nif x < y\n```"),
            (
                "\n\tif #x\n\n      %%y\n\nafter <z>",
                "\n```\nif #x\n\n  %%y\n```\n\nafter &lt;z&gt;",
            ),
            ("    a ``` b", "````\na ``` b\n````"),
            ("    a\n    ```\n    b\n    ```", "```\na\n```\n```\nb\n```"),
            ("   not code\n    lazy", "   not code\n    lazy"),
            ("text\n\n    later", "text\n\n    later"),
        ] {
            assert_eq!(markdown(text), want, "text: {text:?}");
        }
    }

    #[test]
    fn indented_code_after_prose_stays_literal() {
        for (text, want) in [
            (
                "a\n\n    if x < y\n\n    #z\nafter <z>",
                "a\n\n    if x < y\n\n    #z\nafter &lt;z&gt;",
            ),
            ("a\n\n\tx < y", "a\n\n\tx < y"),
            ("a\n    lazy <x>", "a\n    lazy &lt;x&gt;"),
            ("- item\n\n    more <x>", "- item\n\n    more &lt;x&gt;"),
            ("- item\n\n      x < y", "- item\n\n      x < y"),
            ("10. item\n\n       x <y>", "10. item\n\n       x &lt;y&gt;"),
            (
                "1. a\n   - b\n\n        c <d>",
                "1. a\n   - b\n\n        c &lt;d&gt;",
            ),
            (
                "1. a\n   - b\n\n         x < y",
                "1. a\n   - b\n\n         x < y",
            ),
            ("- item\n\nback\n\n    x < y", "- item\n\nback\n\n    x < y"),
            ("```\nf\n```\n    x < y", "```\nf\n```\n    x < y"),
            ("- - item\n\n      <x>", "- - item\n\n      &lt;x&gt;"),
            ("1. - item\n\n         x < y", "1. - item\n\n         x < y"),
            ("a\n2. b\n\n    x < y", "a\n2. b\n\n    x < y"),
            ("a\n1. b\n\n    <x>", "a\n1. b\n\n    &lt;x&gt;"),
            ("1. a\n2. b\n\n    <x>", "1. a\n2. b\n\n    &lt;x&gt;"),
            ("10. a\n2) b\n\n       <x>", "10. a\n2) b\n\n       <x>"),
            (
                "- a\n    lazy\nback\n\n    <x>",
                "- a\n    lazy\nback\n\n    &lt;x&gt;",
            ),
            ("a\n-     b\n\n    <x>", "a\n-     b\n\n    &lt;x&gt;"),
            (
                "- a\n\n      <c>\nback\n\n    x < y",
                "- a\n\n      <c>\nback\n\n    x < y",
            ),
            ("a\n-\n\n    x < y", "a\n\\-\n\n    x < y"),
        ] {
            assert_eq!(markdown(text), want, "text: {text:?}");
        }
    }

    fn check(cases: &[(&str, &str)]) {
        for &(input, want) in cases {
            assert_eq!(markdown(input), want, "input: {input:?}");
        }
    }

    #[test]
    fn fences_are_closed_and_dedented() {
        check(&[
            ("~~~\n<a>\n~~~", "~~~\n<a>\n~~~"),
            ("````\n```\n<a>\n```\n````", "````\n```\n<a>\n```\n````"),
            ("  ```sh\n  ls <dir>\n  ```", "```sh\nls <dir>\n```"),
            (
                "1. step\n    ```sh\n    ls <dir>\n    ```\n2. next",
                "1. step\n```sh\nls <dir>\n```\n2. next",
            ),
            ("```yaml\nkey: 1\n---\n```", "```yaml\nkey: 1\n---\n```"),
            (
                "text\n```rust\nfn main() {}",
                "text\n```rust\nfn main() {}\n```",
            ),
            ("~~~~\nx", "~~~~\nx\n~~~~"),
            ("```\nx\n", "```\nx\n```"),
            ("```\nx\n\n", "```\nx\n\n```"),
            ("```\r\n<a>\r\n```\r\nb", "```\n<a>\n```\nb"),
        ]);
    }

    #[test]
    fn closer_indent_follows_commonmark() {
        check(&[
            (
                "```\ncode\n    ```\n<tag>\n```\nafter <x>",
                "````\ncode\n    ```\n<tag>\n````\nafter &lt;x&gt;",
            ),
            (
                "```\ncode\n    ```\nafter <x>",
                "````\ncode\n    ```\nafter <x>\n````",
            ),
            ("```\nx\n\t```\ny", "````\nx\n\t```\ny\n````"),
            ("```\nx\n   ```\ny <z>", "```\nx\n```\ny &lt;z&gt;"),
            ("  ```\n  a\n     ```\nb", "```\na\n```\nb"),
        ]);
    }

    #[test]
    fn content_cannot_close_the_emitted_fence() {
        check(&[(
            " \t```\ncode\n\t    ```\n<script>x</script>\n \t```\nafter <x>",
            "````\ncode\n   ```\n<script>x</script>\n````\nafter &lt;x&gt;",
        )]);
    }

    #[test]
    fn only_commonmark_whitespace_is_blank() {
        check(&[
            (
                "```\nx\n```\u{a0}\n<t>\n```\nafter <x>",
                "````\nx\n```\u{a0}\n<t>\n````\nafter &lt;x&gt;",
            ),
            ("a\n---\u{a0}", "a\n---\u{a0}"),
            ("\u{a0}\n---", "\u{a0}\n***"),
        ]);
    }

    #[test]
    fn empty_and_blank_text() {
        assert_eq!(markdown(""), "");
        assert_eq!(markdown(" \n "), " \n ");
    }

    #[test]
    fn cc2md_golden_cases() {
        check(&[
            (
                "the trigger (\"let's work on @<task>\")",
                "the trigger (\"let's work on @&lt;task&gt;\")",
            ),
            (
                "are you sure it's not the @<task> tag?",
                "are you sure it's not the @&lt;task&gt; tag?",
            ),
            (
                "resolve `<path>` against root",
                "resolve `<path>` against root",
            ),
            (
                "before\n```\n<task> not html here\n```\nafter <task> here",
                "before\n```\n<task> not html here\n```\nafter &lt;task&gt; here",
            ),
            ("first\n---\nsecond", "first\n***\nsecond"),
        ]);
    }

    #[test]
    fn cc2md_bugs_are_fixed() {
        check(&[
            // A lone backtick does not disable escaping in later paragraphs.
            (
                "it's a ` tick\n\nthen <x>",
                "it's a ` tick\n\nthen &lt;x&gt;",
            ),
            ("~~~\n<a>\n~~~\n<b>", "~~~\n<a>\n~~~\n&lt;b&gt;"),
            (
                "````\n```\n<a>\n```\n````\n<b>",
                "````\n```\n<a>\n```\n````\n&lt;b&gt;",
            ),
            (
                "1. step\n    ```sh\n    ls <dir>\n    ```\n2. next <x>",
                "1. step\n```sh\nls <dir>\n```\n2. next &lt;x&gt;",
            ),
            ("a\n---", "a\n***"),
            (r"match \<word\>", r"match \\&lt;word\\&gt;"),
            (r"even \\<x>", r"even \\&lt;x&gt;"),
        ]);
    }

    #[test]
    fn tags_are_escaped_outside_code() {
        check(&[
            ("#todo at start", r"\#todo at start"),
            ("see #todo and\t#done", "see \\#todo and\t\\#done"),
            ("issue #123", r"issue \#123"),
            ("path #a/b", r"path \#a/b"),
            ("  #indented", r"  \#indented"),
            ("#日本", r"\#日本"),
            ("# Heading\n## Sub", "### Heading\n#### Sub"),
            (
                "foo#bar and https://x.io/a#frag",
                "foo#bar and https://x.io/a#frag",
            ),
            ("#!/bin/sh", "#!/bin/sh"),
            ("use `#tag` here", "use `#tag` here"),
            ("```\n#tag\n```", "```\n#tag\n```"),
        ]);
    }

    #[test]
    fn headings_are_demoted() {
        check(&[
            ("# Title", "### Title"),
            ("## Sub\n### Deep", "#### Sub\n##### Deep"),
            ("##### five\n###### six", "###### five\n###### six"),
            ("   # indented", "   ### indented"),
            ("#", "###"),
            ("#\tTab", "###\tTab"),
            ("# a #tag <x>", r"### a \#tag &lt;x&gt;"),
            ("#tag", r"\#tag"),
            ("####### seven", "####### seven"),
            ("```\n# not\n```", "```\n# not\n```"),
            ("a\n\n    # code", "a\n\n    # code"),
        ]);
    }

    #[test]
    fn headings_in_list_items_are_demoted() {
        check(&[
            ("1. item\n\n    # Sub", "1. item\n\n    ### Sub"),
            ("- a\n\n\t# T", "- a\n\n\t### T"),
            ("- # H", "- ### H"),
            ("1. - # H <x>", "1. - ### H &lt;x&gt;"),
            ("- a\n  ## b", "- a\n  #### b"),
            ("- a\n\n # b", "- a\n\n ### b"),
            ("- a\n      # b", "- a\n      # b"),
            ("- a\n\n      # b", "- a\n\n      # b"),
            ("a\n2. # b", "a\n2. # b"),
            ("-     # code", "-     # code"),
        ]);
    }

    #[test]
    fn markup_after_a_rule_stays_escaped() {
        check(&[
            (
                "- a\n---\n# h\n    <img src=x onerror=alert(1)>",
                "- a\n***\n### h\n    <img src=x onerror=alert(1)>",
            ),
            ("a\n---\n# h\n    <x>", "a\n***\n### h\n    <x>"),
            (
                "a\n---\n- # b\n      <b>x</b>",
                "a\n***\n- ### b\n      <b>x</b>",
            ),
            (
                "a\n---\n```\n<iframe src=x></iframe>\n```",
                "a\n***\n```\n<iframe src=x></iframe>\n```",
            ),
        ]);
    }

    #[test]
    fn an_empty_item_after_a_heading_is_not_an_underline() {
        check(&[
            (
                "# h\n-\n\t# t\n      <b>x</b>",
                "### h\n-\n\t### t\n      <b>x</b>",
            ),
            (
                "  ## h\n-\n    # h\n\t# h\n      <x>",
                "  #### h\n-\n    ### h\n\t### h\n      <x>",
            ),
            (
                "   # h\n-\n\t# h\n      <x>",
                "   ### h\n-\n\t### h\n      <x>",
            ),
        ]);
    }

    #[test]
    fn rules_and_bare_items_end_the_paragraph() {
        check(&[
            (
                "  ---\n2. a\n\n      <b>x</b>",
                "  ---\n2. a\n\n      &lt;b&gt;x&lt;/b&gt;",
            ),
            ("---\n2. b\n      # h", "---\n2. b\n      ### h"),
            ("-\n2. b\n      # h", "-\n2. b\n      ### h"),
            ("-     # c\n2. b\n      # h", "-     # c\n2. b\n      ### h"),
            ("- * * *\n    <x>", "- * * *\n    &lt;x&gt;"),
            ("- - -\n    <x>", "- - -\n    <x>"),
        ]);
    }

    #[test]
    fn items_close_where_the_line_leaves_them() {
        check(&[
            (
                "- a\n2. a\n\t# h\n      <b>x</b>",
                "- a\n2. a\n\t### h\n      &lt;b&gt;x&lt;/b&gt;",
            ),
            (
                "1. a\n   123456789. b\n\n        x<y>",
                "1. a\n   123456789. b\n\n        x<y>",
            ),
        ]);
    }

    /// Every short mix of block-level lines, read back by a CommonMark parser.
    #[test]
    fn no_line_mix_lets_raw_html_or_a_top_heading_through() {
        use pulldown_cmark::{Event, HeadingLevel, Parser, Tag};
        const LINES: [&str; 22] = [
            "",
            "a",
            "# h",
            "  # h",
            "    # h",
            "\t# h",
            "-",
            "- a",
            "- # h",
            "1. a",
            "2. a",
            "---",
            "  ---",
            "===",
            "```",
            "<x>",
            "    <x>",
            "      <b>x</b>",
            ">",
            "> a",
            "> ```",
            "    > a",
        ];
        let mut stack = vec![Vec::<&str>::new()];
        while let Some(prefix) = stack.pop() {
            if prefix.len() < 4 {
                for line in LINES {
                    let mut next = prefix.clone();
                    next.push(line);
                    stack.push(next);
                }
            }
            let text = prefix.join("\n");
            let out = markdown(&text);
            let bad = Parser::new(&out).find(|e| {
                matches!(
                    e,
                    Event::Html(_)
                        | Event::InlineHtml(_)
                        | Event::Start(Tag::Heading {
                            level: HeadingLevel::H1 | HeadingLevel::H2,
                            ..
                        })
                )
            });
            assert!(bad.is_none(), "input: {text:?}\noutput: {out:?}\n{bad:?}");
        }
    }

    #[test]
    fn a_heading_ends_the_paragraph_and_list() {
        check(&[
            ("# H\n    x < y", "### H\n    x < y"),
            ("- a\n# b\n\n    x < y", "- a\n### b\n\n    x < y"),
            ("- a\n  # b\n      x < y", "- a\n  ### b\n      x < y"),
        ]);
    }

    #[test]
    fn multibyte_text_around_escapes() {
        check(&[("é<x>é #ü", "é&lt;x&gt;é \\#ü")]);
    }

    #[test]
    fn setext_underlines_are_neutralised() {
        check(&[
            ("a\n---\nb", "a\n***\nb"),
            ("a\n  -----  ", "a\n  ***"),
            ("a\n\n---\nb", "a\n\n---\nb"),
            ("---\nb", "---\nb"),
            ("```\nx\n```\n---", "```\nx\n```\n---"),
            ("a\n--", "a\n\\--"),
            ("a\n===", "a\n\\==="),
            ("a\n    ---", "a\n    ---"),
            ("a\r\n---\r\nb", "a\n***\nb"),
            ("- a\n    ---", "- a\n    ***"),
            ("- a\n  ==", "- a\n  \\=="),
            ("- a\n---\n\n    <x>", "- a\n***\n\n    <x>"),
        ]);
    }

    #[test]
    fn a_quote_ends_the_list() {
        check(&[
            // The quote closes the list, so after a blank the indented line is code.
            ("- a\n> q\n\n    x < y", "- a\n> q\n\n    x < y"),
            // Indented into the item, the quote keeps the list open: continuation text.
            ("- a\n  > q\n\n    x < y", "- a\n  > q\n\n    x &lt; y"),
        ]);
    }

    #[test]
    fn a_line_after_a_quote_starts_fresh() {
        check(&[
            ("> a\nb", "> a\n\nb"),
            ("> a\n    x<y>", "> a\n\n    x<y>"),
            ("> ```\n> ```\n2. a\n\t# h", "> ```\n> ```\n\n2. a\n\t### h"),
            ("- a\n  > q\n  b", "- a\n  > q\n\n  b"),
            ("> a\n---", "> a\n\n---"),
            ("> a\n> b\n\nc", "> a\n> b\n\nc"),
            ("> a\n```\nx\n```", "> a\n```\nx\n```"),
            ("> a\n    > x\n2. a\n\t# h", "> a\n\n    > x\n2. a\n\t### h"),
            (
                "> ```\n    > x\n2. a\n\t# h",
                "> ```\n\n    > x\n2. a\n\t### h",
            ),
            (
                "> a\n> ```\n    > x\n2. a\n\t# h",
                "> a\n> ```\n\n    > x\n2. a\n\t### h",
            ),
        ]);
    }

    #[test]
    fn quote_markers_are_kept() {
        check(&[
            ("> a", "> a"),
            ("> > a\n>> b\n> >c", "> > a\n>> b\n> >c"),
            ("> a\n>\n> b", "> a\n>\n> b"),
            ("  > a", "  > a"),
            (
                "> | a | b |\n> | --- | --- |\n> | 1 | 2 |",
                "> | a | b |\n> | --- | --- |\n> | 1 | 2 |",
            ),
            ("a > b <c>", "a &gt; b &lt;c&gt;"),
            ("> a > b", "> a &gt; b"),
            ("> #tag %%x%%", "> \\#tag \\%\\%x\\%\\%"),
            ("> a `x>y`", "> a `x>y`"),
        ]);
    }

    #[test]
    fn quoted_callout_openers_are_escaped() {
        check(&[
            ("> [!warning] x", "> \\[!warning] x"),
            (">[!note] x", ">\\[!note] x"),
            ("> > [!tip] x", "> > \\[!tip] x"),
            (">>[!tip] x", ">>\\[!tip] x"),
            (">   [!x] y", ">   \\[!x] y"),
            ("> a\n> [!x] y", "> a\n> \\[!x] y"),
            ("> [x] y", "> [x] y"),
            (">\u{a0}[!warning] x", ">\u{a0}\\[!warning] x"),
            (">\u{3000}[!x] y", ">\u{3000}\\[!x] y"),
            (">\t[!x] y", ">\t\\[!x] y"),
            ("[!x](http://e.com) y", "[!x](http://e.com) y"),
        ]);
    }

    #[test]
    fn code_spans_stay_on_one_line() {
        check(&[
            ("a ` b\n---\n<x>`", "a ` b\n***\n&lt;x&gt;`"),
            ("a ` b\n# H <x> `", "a ` b\n### H &lt;x&gt; `"),
            ("a ` b\n- item <x> `", "a ` b\n- item &lt;x&gt; `"),
            ("a ` b\n2) item <x> `", "a ` b\n2) item &lt;x&gt; `"),
            ("a ` b\n> q <x> `", "a ` b\n> q &lt;x&gt; `"),
            ("# H `\n<task>`", "### H `\n&lt;task&gt;`"),
            (
                "before `\n<div>\n</div>\n`",
                "before `\n&lt;div&gt;\n&lt;/div&gt;\n`",
            ),
            ("a ` b\n***\n<x> `", "a ` b\n***\n&lt;x&gt; `"),
            // A span wrapped across lines shows its escapes, but hides nothing.
            ("a `x\n<y>` b", "a `x\n&lt;y&gt;` b"),
            ("a ``x ` <y>`` <z>", "a ``x ` <y>`` &lt;z&gt;"),
        ]);
    }

    #[test]
    fn comment_markers_are_escaped_outside_code() {
        check(&[
            (
                "for %%i in (*) do echo %%i",
                r"for \%\%i in (*) do echo \%\%i",
            ),
            ("100% sure", "100% sure"),
            ("a %%% b", r"a \%\%\% b"),
            (r"50\%% done", r"50\%\% done"),
            (r"50%\% done", r"50%\% done"),
            (r"even \\%%", r"even \\\%\%"),
            ("use `%%i` here", "use `%%i` here"),
            ("```bat\necho %%i\n```", "```bat\necho %%i\n```"),
        ]);
    }

    #[test]
    fn escaped_backticks_do_not_open_a_span() {
        check(&[
            (r"use \`<x>\` here", r"use \`&lt;x&gt;\` here"),
            (r"\``<x>`", r"\``<x>`"),
            (r"\\`<x>`", r"\\`<x>`"),
            (r"`a\` <x>", r"`a\` &lt;x&gt;"),
        ]);
    }

    #[test]
    fn unmatched_backtick_runs_scan_in_linear_time() {
        let text: String = (1..=3000).map(|n| "`".repeat(n) + " ").collect();
        let start = std::time::Instant::now();
        assert_eq!(markdown(&text), text);
        let elapsed = start.elapsed();
        // Generous for slow CI runners; a quadratic scan takes minutes here.
        assert!(elapsed.as_secs() < 10, "took {elapsed:?}");
    }

    #[test]
    fn inline_triple_backticks_are_not_a_fence() {
        check(&[("```ls <dir>```\nthen <x>", "```ls <dir>```\nthen &lt;x&gt;")]);
    }

    #[test]
    fn bare_carriage_returns_end_lines() {
        check(&[
            (
                "before `\r<div>\r</div>\r`",
                "before `\n&lt;div&gt;\n&lt;/div&gt;\n`",
            ),
            ("```\r<a>\r```\rb <x>", "```\n<a>\n```\nb &lt;x&gt;"),
        ]);
    }

    #[test]
    fn crlf_prose_is_escaped() {
        check(&[("```\r\n<a>\r\n```\r\nb <x>", "```\n<a>\n```\nb &lt;x&gt;")]);
    }

    #[test]
    fn label_is_one_line_with_punctuation_escaped() {
        assert_eq!(label("Explore"), "Explore");
        assert_eq!(label("fix\n# heading  now"), "fix \\# heading now");
        assert_eq!(label("a*b_[[c]]"), "a\\*b\\_\\[\\[c\\]\\]");
        assert_eq!(label("<x> `y` \\"), "\\<x\\> \\`y\\` \\\\");
    }
}
