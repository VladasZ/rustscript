//! A focused docx bridge backed by xmltree. It fills the fixed four-table
//! invoice layout in a word/document.xml, taking the cell texts already
//! formatted by the make_invoice script. The money and quantity formatting and
//! all CLI work stay in the script, this bridge is only the XML DOM edit that a
//! plain string rewrite cannot do safely.

use xmltree::{Element, EmitterConfig, XMLNode};

use super::value::Value;

pub(super) fn edit_document_xml(xml: &str, opts_json: &str) -> Value {
    match do_edit(xml, opts_json) {
        Ok(out) => Value::ok(Value::str(out)),
        Err(e) => Value::err(Value::str(e)),
    }
}

fn qname(e: &Element) -> String {
    match &e.prefix {
        Some(p) => format!("{p}:{}", e.name),
        None => e.name.clone(),
    }
}

fn child_elem_indices(parent: &Element, tag: &str) -> Vec<usize> {
    parent
        .children
        .iter()
        .enumerate()
        .filter_map(|(i, n)| match n {
            XMLNode::Element(e) if qname(e) == tag => Some(i),
            _ => None,
        })
        .collect()
}

fn is_run_content(q: &str) -> bool {
    matches!(q, "w:t" | "w:br" | "w:cr" | "w:tab" | "w:noBreakHyphen")
}

fn make_wt(text: &str) -> Element {
    let mut t = Element::new("w:t");
    t.attributes
        .insert("xml:space".to_string(), "preserve".to_string());
    t.children.push(XMLNode::Text(text.to_string()));
    t
}

// Drop a run's content children, keeping its w:rPr formatting, then set the text
// as a single w:t when it is not empty.
fn set_run_text(run: &mut Element, text: &str) {
    run.children
        .retain(|n| !matches!(n, XMLNode::Element(e) if is_run_content(&qname(e))));
    if !text.is_empty() {
        run.children.push(XMLNode::Element(make_wt(text)));
    }
}

// Set the cell's first run text, blank the other runs, and drop the cell's
// extra paragraphs so old template values do not carry over.
fn replace_first_run(cell: &mut Element, text: &str) {
    let para_idxs = child_elem_indices(cell, "w:p");
    let Some(&first_para) = para_idxs.first() else {
        return;
    };
    if let XMLNode::Element(para) = &mut cell.children[first_para] {
        let run_idxs = child_elem_indices(para, "w:r");
        if run_idxs.is_empty() {
            let mut run = Element::new("w:r");
            run.children.push(XMLNode::Element(make_wt(text)));
            para.children.push(XMLNode::Element(run));
        } else {
            for (n, &ri) in run_idxs.iter().enumerate() {
                if let XMLNode::Element(run) = &mut para.children[ri] {
                    set_run_text(run, if n == 0 { text } else { "" });
                }
            }
        }
    }
    let mut kept_first = false;
    cell.children.retain(|n| match n {
        XMLNode::Element(e) if qname(e) == "w:p" => {
            if kept_first {
                false
            } else {
                kept_first = true;
                true
            }
        }
        _ => true,
    });
}

fn set_cell_text(row: &mut Element, cell_idx: usize, text: &str) {
    if let XMLNode::Element(cell) = &mut row.children[cell_idx] {
        replace_first_run(cell, text);
    }
}

fn leading_decl(xml: &str) -> String {
    let trimmed = xml.trim_start();
    if trimmed.starts_with("<?xml")
        && let Some(end) = trimmed.find("?>")
    {
        return trimmed[..end + 2].to_string();
    }
    String::new()
}

fn line_cell(line: &serde_json::Value, i: usize) -> &str {
    line.get(i)
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
}

fn do_edit(xml: &str, opts_json: &str) -> Result<String, String> {
    let opts: serde_json::Value = serde_json::from_str(opts_json).map_err(|e| e.to_string())?;
    let date = opts
        .get("date")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let invoice_num = opts
        .get("invoice_num")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let due = opts
        .get("due_date")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let subtotal = opts
        .get("subtotal")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let empty: Vec<serde_json::Value> = Vec::new();
    let lines = opts
        .get("lines")
        .and_then(serde_json::Value::as_array)
        .unwrap_or(&empty);

    let mut root = Element::parse(xml.as_bytes()).map_err(|e| e.to_string())?;

    let Some(&body_idx) = child_elem_indices(&root, "w:body").first() else {
        return Err("no w:body in document".to_string());
    };
    let XMLNode::Element(body) = &mut root.children[body_idx] else {
        return Err("w:body missing".to_string());
    };

    let tables = child_elem_indices(body, "w:tbl");
    if tables.len() < 4 {
        return Err(format!(
            "Template should have at least 4 tables (found {})",
            tables.len()
        ));
    }

    // Table 2: date / invoice number / due date, second row.
    if let XMLNode::Element(table) = &mut body.children[tables[1]] {
        let rows = child_elem_indices(table, "w:tr");
        if let Some(&row_idx) = rows.get(1)
            && let XMLNode::Element(row) = &mut table.children[row_idx]
        {
            let cells = child_elem_indices(row, "w:tc");
            if cells.len() >= 3 {
                set_cell_text(row, cells[0], date);
                set_cell_text(row, cells[1], invoice_num);
                set_cell_text(row, cells[2], due);
            }
        }
    }

    // Table 3: line items, row 0 is the header.
    if let XMLNode::Element(table) = &mut body.children[tables[2]] {
        let rows = child_elem_indices(table, "w:tr");
        let capacity = rows.len().saturating_sub(1);
        if lines.len() > capacity {
            return Err(format!(
                "Template has only {capacity} line slots; got {} lines",
                lines.len()
            ));
        }
        for (i, line) in lines.iter().enumerate() {
            if let Some(&row_idx) = rows.get(i + 1)
                && let XMLNode::Element(row) = &mut table.children[row_idx]
            {
                let cells = child_elem_indices(row, "w:tc");
                if cells.len() >= 4 {
                    set_cell_text(row, cells[0], line_cell(line, 0));
                    set_cell_text(row, cells[1], line_cell(line, 1));
                    set_cell_text(row, cells[2], line_cell(line, 2));
                    set_cell_text(row, cells[3], line_cell(line, 3));
                }
            }
        }
        // Wipe the first four columns of any remaining template rows.
        for &row_idx in rows.iter().skip(lines.len() + 1) {
            if let XMLNode::Element(row) = &mut table.children[row_idx] {
                let cells = child_elem_indices(row, "w:tc");
                for &cell_idx in cells.iter().take(4) {
                    set_cell_text(row, cell_idx, "");
                }
            }
        }
    }

    // Table 4: subtotal and total due, both take the computed subtotal.
    if let XMLNode::Element(table) = &mut body.children[tables[3]] {
        let rows = child_elem_indices(table, "w:tr");
        for &wanted in &[0usize, 2usize] {
            if let Some(&row_idx) = rows.get(wanted)
                && let XMLNode::Element(row) = &mut table.children[row_idx]
            {
                let cells = child_elem_indices(row, "w:tc");
                if let Some(&cell_idx) = cells.get(1) {
                    set_cell_text(row, cell_idx, subtotal);
                }
            }
        }
    }

    let mut buf: Vec<u8> = Vec::new();
    let config = EmitterConfig::new()
        .perform_indent(false)
        .write_document_declaration(false);
    root.write_with_config(&mut buf, config)
        .map_err(|e| e.to_string())?;
    let rendered = String::from_utf8(buf).map_err(|e| e.to_string())?;
    Ok(format!("{}{rendered}", leading_decl(xml)))
}
