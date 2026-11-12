// Copyright 2026 Haydn Trowell
// SPDX-FileCopyrightText: 2026 Haydn Trowell
//
// SPDX-License-Identifier: GPL-3.0-or-later

//! Walks typst_html::HtmlDocument and emits Markdown.

use typst_html::{HtmlAttr, HtmlDocument, HtmlElement, HtmlNode};

pub fn document_to_markdown(doc: &HtmlDocument) -> String {
    let mut out = String::new();
    walk_element(doc.root(), &mut out, "");
    normalize_blank_lines(&out)
}

/// Collapses any run of 3+ consecutive newlines down to 2.
fn normalize_blank_lines(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut newline_run = 0;
    for c in s.chars() {
        if c == '\n' {
            newline_run += 1;
            if newline_run <= 2 {
                out.push(c);
            }
        } else {
            newline_run = 0;
            out.push(c);
        }
    }
    let trimmed = out.trim_matches('\n');
    if trimmed.is_empty() {
        String::new()
    } else {
        format!("{trimmed}\n")
    }
}

/// Interns an attribute name at the point of use.
fn attr(name: &str) -> HtmlAttr {
    HtmlAttr::intern(name).expect("attribute name is a valid HTML identifier")
}

fn get_attr<'a>(el: &'a HtmlElement, name: &str) -> Option<&'a str> {
    el.attrs.get(attr(name)).map(|s| s.as_str())
}

fn walk_element(el: &HtmlElement, out: &mut String, indent: &str) {
    let tag = el.tag.resolve();

    match tag.as_str() {
        "h1" => emit_heading(el, out, 1),
        "h2" => emit_heading(el, out, 2),
        "h3" => emit_heading(el, out, 3),
        "h4" => emit_heading(el, out, 4),
        "h5" => emit_heading(el, out, 5),
        "h6" => emit_heading(el, out, 6),
        "p" => {
            walk_children(el, out, indent);
            out.push_str("\n\n");
        }
        "strong" | "b" => wrap_inline(el, out, indent, "**"),
        "em" | "i" => wrap_inline(el, out, indent, "_"),
        "s" | "del" | "strike" => wrap_inline(el, out, indent, "~~"),
        "code" => {
            // Inline code (not inside pre).
            let mut text = String::new();
            walk_pre_text(el, &mut text);
            out.push('`');
            out.push_str(&text);
            out.push('`');
        }
        "pre" => {
            let lang = find_pre_code_lang(el).unwrap_or_default();
            out.push_str("```");
            out.push_str(lang);
            out.push('\n');
            walk_pre_text(el, out);
            out.push_str("\n```\n\n");
        }
        "a" if matches!(
            get_attr(el, "role"),
            Some("doc-biblioref") | Some("doc-backlink")
        ) =>
        {
            // Citation link. Drop the link but keep visible text unchanged.
            walk_children(el, out, indent);
        }
        "a" => {
            out.push('[');
            walk_children(el, out, indent);
            out.push_str("](");
            push_url(out, get_attr(el, "href").unwrap_or(""));
            push_title(out, get_attr(el, "title"));
            out.push(')');
        }
        "sup" if get_attr(el, "role") == Some("doc-noteref") => {
            // Footnote reference. Pull out just the marker text.
            let mut marker = String::new();
            walk_pre_text(el, &mut marker);
            out.push('^');
            out.push_str(&marker);
            out.push('^');
        }
        "sup" if get_attr(el, "role") == Some("doc-backlink") => {
            // Backlink inside a footnote. Drop entirely.
        }
        "sup" => wrap_inline(el, out, indent, "^"),
        "sub" => wrap_inline(el, out, indent, "~"),
        "img" => {
            let alt = get_attr(el, "alt").unwrap_or("");
            out.push_str("![");
            out.push_str(alt);
            out.push_str("](");
            push_url(out, get_attr(el, "src").unwrap_or(""));
            push_title(out, get_attr(el, "title"));
            out.push(')');
        }
        "ul" => {
            walk_list_items(el, out, indent, None);
            if indent.is_empty() {
                out.push('\n');
            }
        }
        "ol" => {
            // Honor a non-default start, e.g. <ol start="5">.
            let start = get_attr(el, "start")
                .and_then(|s| s.parse::<usize>().ok())
                .unwrap_or(1);
            walk_list_items(el, out, indent, Some(start));
            if indent.is_empty() {
                out.push('\n');
            }
        }
        "blockquote" => {
            for line in {
                let mut inner = String::new();
                walk_children(el, &mut inner, indent);
                inner
            }
            .lines()
            {
                out.push_str("> ");
                out.push_str(line);
                out.push('\n');
            }
            out.push('\n');
        }
        "math" => {
            let markup = serialize_element_as_html(el);
            if get_attr(el, "display") == Some("block") {
                out.push('\n');
                out.push_str(&markup);
                out.push_str("\n\n");
            } else {
                out.push_str(&markup);
            }
        }
        "dl" => {
            walk_children(el, out, indent);
        }
        "dt" => {
            walk_children(el, out, indent);
            out.push('\n');
        }
        "dd" => {
            out.push_str(":   ");
            walk_children(el, out, indent);
            out.push_str("\n\n");
        }
        "table" => {
            walk_table(el, out);
        }
        "br" => {
            out.push_str("\\\n");
        }
        "figcaption" => {
            out.push_str("\n\n*");
            walk_children(el, out, indent);
            out.push_str("*\n\n");
        }
        "hr" => {
            out.push_str("\n\n---\n\n");
        }
        "section" if get_attr(el, "role") == Some("doc-endnotes") => {
            out.push_str("\n\n---\n\n");
            walk_children(el, out, indent);
        }
        "head" | "style" | "script" | "title" => {
            // Non-content metadata. Mustn't fall through to the catch-all below.
        }
        _ => {
            // Unknown/structural tag (html, body, div, etc.). Recurse without emitting markup.
            walk_children(el, out, indent);
        }
    }
}

fn emit_heading(el: &HtmlElement, out: &mut String, level: usize) {
    out.push_str(&"#".repeat(level));
    out.push(' ');
    walk_children(el, out, "");
    out.push_str("\n\n");
}

fn wrap_inline(el: &HtmlElement, out: &mut String, indent: &str, marker: &str) {
    out.push_str(marker);
    walk_children(el, out, indent);
    out.push_str(marker);
}

/// Writes a Markdown link/image destination.
fn push_url(out: &mut String, url: &str) {
    let needs_angle_brackets = url.chars().any(|c| c.is_whitespace() || c == '(' || c == ')');
    if !needs_angle_brackets {
        out.push_str(url);
        return;
    }
    out.push('<');
    for c in url.chars() {
        if c == '<' || c == '>' {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('>');
}

/// Appends a Markdown title suffix (` "title"`) for a link/image destination.
fn push_title(out: &mut String, title: Option<&str>) {
    if let Some(title) = title {
        out.push_str(" \"");
        out.push_str(&title.replace('"', "\\\""));
        out.push('"');
    }
}

/// Escapes characters in plain prose text.
fn escape_markdown_text(text: &str, mut at_line_start: bool) -> String {
    let mut escaped = String::with_capacity(text.len());
    for ch in text.chars() {
        let needs_escape = matches!(ch, '\\' | '`' | '*' | '_' | '[' | ']' | '<' | '>' | '~' | '^')
            || (at_line_start && matches!(ch, '#' | '-' | '+' | '='));
        if needs_escape {
            escaped.push('\\');
        }
        escaped.push(ch);
        at_line_start = ch == '\n';
    }
    escaped
}

/// Finds the data-lang attribute on a <pre>'s inner <code> element, if present.
fn find_pre_code_lang(pre: &HtmlElement) -> Option<&str> {
    pre.children.iter().find_map(|child| {
        if let HtmlNode::Element(code_el) = child
            && code_el.tag.resolve().as_str() == "code"
        {
            get_attr(code_el, "data-lang")
        } else {
            None
        }
    })
}

/// Extracts plain text content, ignoring wrapper elements.
fn walk_pre_text(el: &HtmlElement, out: &mut String) {
    for child in &el.children {
        match child {
            HtmlNode::Text(text, _) => out.push_str(text),
            HtmlNode::Element(child_el) => walk_pre_text(child_el, out),
            HtmlNode::Tag(_) | HtmlNode::Frame(_) => {}
        }
    }
}

fn walk_list_items(el: &HtmlElement, out: &mut String, indent: &str, mut numbered: Option<usize>) {
    for child in &el.children {
        if let HtmlNode::Element(li) = child
            && li.tag.resolve().as_str() == "li"
        {
            out.push_str(indent);
            let marker = match &mut numbered {
                Some(n) => {
                    let m = format!("{}. ", n);
                    *n += 1;
                    m
                }
                None => "- ".to_string(),
            };
            out.push_str(&marker);
            let child_indent = format!("{indent}{}", " ".repeat(marker.len()));
            let mut item = String::new();
            walk_children(li, &mut item, &child_indent);
            out.push_str(item.trim_start());
            out.push('\n');
        }
    }
}

/// A rowspan cell that's still owed content in one or more rows below the
/// one it was declared in.
struct PendingSpan {
    col: usize,
    rows_remaining: usize,
    text: String,
}

#[derive(Clone, Copy)]
enum Align {
    Left,
    Center,
    Right,
}

/// Reads a cell's alignment from a plain `align="..."` attribute or a
/// `style="text-align: ..."` declaration, whichever is present.
fn cell_alignment(cell_el: &HtmlElement) -> Option<Align> {
    let value = get_attr(cell_el, "align").or_else(|| {
        get_attr(cell_el, "style").and_then(|style| {
            style.split(';').find_map(|decl| {
                let (prop, val) = decl.split_once(':')?;
                prop.trim()
                    .eq_ignore_ascii_case("text-align")
                    .then(|| val.trim())
            })
        })
    })?;
    match value.to_ascii_lowercase().as_str() {
        "left" => Some(Align::Left),
        "center" => Some(Align::Center),
        "right" => Some(Align::Right),
        _ => None,
    }
}

/// The Markdown separator-row token for a column's alignment.
fn align_token(align: Option<Align>) -> &'static str {
    match align {
        Some(Align::Left) => ":---",
        Some(Align::Center) => ":---:",
        Some(Align::Right) => "---:",
        None => "---",
    }
}

fn walk_table(el: &HtmlElement, out: &mut String) {
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut pending: Vec<PendingSpan> = Vec::new();
    let mut alignments: Vec<Option<Align>> = Vec::new();
    collect_table_rows(el, &mut rows, &mut pending, &mut alignments);

    if rows.is_empty() {
        return;
    }

    let col_count = rows.iter().map(Vec::len).max().unwrap_or(0);
    if col_count == 0 {
        return;
    }
    let render_row = |row: &[String]| -> String {
        let mut cells: Vec<&str> = row.iter().map(String::as_str).collect();
        cells.resize(col_count, "");
        cells.join(" | ")
    };
    let separator: String = (0..col_count)
        .map(|i| format!("{}|", align_token(alignments.get(i).copied().flatten())))
        .collect();

    out.push_str(&format!("| {} |\n", render_row(&rows[0])));
    out.push_str(&format!("|{separator}\n"));
    for row in &rows[1..] {
        out.push_str(&format!("| {} |\n", render_row(row)));
    }
    out.push('\n');
}

fn collect_table_rows(
    el: &HtmlElement,
    rows: &mut Vec<Vec<String>>,
    pending: &mut Vec<PendingSpan>,
    alignments: &mut Vec<Option<Align>>,
) {
    for child in &el.children {
        if let HtmlNode::Element(inner) = child {
            let inner_tag = inner.tag.resolve();
            match inner_tag.as_str() {
                "thead" | "tbody" => collect_table_rows(inner, rows, pending, alignments),
                "tr" => {
                    let is_header_row = rows.is_empty();
                    let mut row = Vec::new();
                    let mut cells = inner.children.iter().filter_map(|c| match c {
                        HtmlNode::Element(cell_el) => Some(cell_el),
                        _ => None,
                    });
                    let mut col = 0usize;
                    loop {
                        if let Some(p) = pending.iter_mut().find(|p| p.col == col && p.rows_remaining > 0) {
                            row.push(p.text.clone());
                            p.rows_remaining -= 1;
                            col += 1;
                            continue;
                        }
                        if let Some(cell_el) = cells.next() {
                            let mut cell_text = String::new();
                            walk_children(cell_el, &mut cell_text, "");
                            let cell_text = cell_text.trim().replace('|', "\\|").to_string();
                            let colspan = get_attr(cell_el, "colspan")
                                .and_then(|s| s.parse::<usize>().ok())
                                .filter(|&n| n > 0)
                                .unwrap_or(1);
                            let rowspan = get_attr(cell_el, "rowspan")
                                .and_then(|s| s.parse::<usize>().ok())
                                .filter(|&n| n > 0)
                                .unwrap_or(1);
                            let align = is_header_row.then(|| cell_alignment(cell_el)).flatten();
                            for i in 0..colspan {
                                row.push(cell_text.clone());
                                if is_header_row {
                                    let idx = col + i;
                                    if idx >= alignments.len() {
                                        alignments.resize(idx + 1, None);
                                    }
                                    alignments[idx] = align;
                                }
                                if rowspan > 1 {
                                    pending.push(PendingSpan {
                                        col: col + i,
                                        rows_remaining: rowspan - 1,
                                        text: cell_text.clone(),
                                    });
                                }
                                col += 1;
                            }
                            continue;
                        }
                        if pending.iter().any(|p| p.col > col && p.rows_remaining > 0) {
                            row.push(String::new());
                            col += 1;
                            continue;
                        }
                        break;
                    }
                    rows.push(row);
                    pending.retain(|p| p.rows_remaining > 0);
                }
                _ => collect_table_rows(inner, rows, pending, alignments),
            }
        }
    }
}

fn walk_children(el: &HtmlElement, out: &mut String, indent: &str) {
    for child in &el.children {
        match child {
            HtmlNode::Text(text, _span) => {
                let at_line_start = out.is_empty() || out.ends_with('\n');
                out.push_str(&escape_markdown_text(text, at_line_start));
            }
            HtmlNode::Element(child_el) => walk_element(child_el, out, indent),
            HtmlNode::Tag(_) | HtmlNode::Frame(_) => {} // introspection/SVG — no Markdown output
        }
    }
}

/// Serializes an HtmlElement subtree back to literal HTML/MathML verbatim.
fn serialize_element_as_html(el: &HtmlElement) -> String {
    let mut s = String::new();
    write_element_as_html(el, &mut s);
    s
}

fn write_element_as_html(el: &HtmlElement, out: &mut String) {
    let tag = el.tag.resolve();
    out.push('<');
    out.push_str(tag.as_str());
    for (attr, value) in el.attrs.0.iter() {
        let name = attr.resolve();
        if matches!(name.as_str(), "lspace" | "rspace") {
            continue;
        }
        out.push(' ');
        out.push_str(name.as_str());
        out.push_str("=\"");
        out.push_str(&escape_attr_value(value));
        out.push('"');
    }
    out.push('>');
    for child in &el.children {
        match child {
            HtmlNode::Text(text, _) => out.push_str(&escape_html_text(text)),
            HtmlNode::Element(child_el) => write_element_as_html(child_el, out),
            HtmlNode::Tag(_) | HtmlNode::Frame(_) => {} // no MathML equivalent, dropped
        }
    }
    out.push_str("</");
    out.push_str(tag.as_str());
    out.push('>');
}

fn escape_html_text(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn escape_attr_value(value: &str) -> String {
    value.replace('&', "&amp;").replace('"', "&quot;")
}