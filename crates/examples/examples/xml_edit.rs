//! Parse an XML document with xmltree, edit the tree, and serialize it back.
//! Namespaces, attributes, and node order survive the round trip.

use xmltree::{Element, XMLNode};

fn main() -> anyhow::Result<()> {
    let source = r#"<?xml version="1.0" encoding="UTF-8"?><w:doc xmlns:w="http://example.com/word"><w:body note="keep"><w:p>old text</w:p><w:p>second</w:p></w:body></w:doc>"#;

    let mut doc = Element::parse(source.as_bytes())?;
    println!(
        "root {} prefix {}",
        doc.name,
        doc.prefix.clone().unwrap_or_default()
    );

    for node in &mut doc.children {
        if let XMLNode::Element(body) = node {
            println!("body keeps note = {}", body.attributes["note"]);
            body.attributes
                .insert("edited".to_string(), "yes".to_string());
            let mut paragraphs = 0;
            for child in &mut body.children {
                if let XMLNode::Element(p) = child {
                    paragraphs += 1;
                    if paragraphs == 1 {
                        println!("first paragraph was: {}", p.get_text().unwrap_or_default());
                        p.children = Vec::new();
                        p.children.push(XMLNode::Text("new text".to_string()));
                    }
                }
            }
            println!("paragraphs: {paragraphs}");
        }
    }

    let mut note = Element::new("w:note");
    note.children.push(XMLNode::Text("added".to_string()));
    doc.children.push(XMLNode::Element(note));

    let mut out: Vec<u8> = Vec::new();
    doc.write(&mut out)?;
    println!("{}", String::from_utf8(out)?);
    Ok(())
}
