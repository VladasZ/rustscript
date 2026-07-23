//! Build a tiny PDF by hand, then use lopdf to load it, read a page's content
//! stream, rewrite it, save, and read the result back. The xref offsets are
//! computed while the file is assembled, so the fixture needs no data file.

use lopdf::Document;

fn pad10(n: usize) -> String {
    let s = n.to_string();
    "0".repeat(10 - s.len()) + &s
}

fn add_obj(pdf: &mut String, offsets: &mut Vec<usize>, body: &str) {
    offsets.push(pdf.len());
    pdf.push_str(body);
}

fn build_mini_pdf() -> String {
    let mut pdf = String::new();
    let mut offsets: Vec<usize> = Vec::new();
    pdf.push_str("%PDF-1.4\n");
    add_obj(
        &mut pdf,
        &mut offsets,
        "1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n",
    );
    add_obj(
        &mut pdf,
        &mut offsets,
        "2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n",
    );
    add_obj(
        &mut pdf,
        &mut offsets,
        "3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R >>\nendobj\n",
    );
    let stream = "BT /F1 12 Tf 72 720 Td (Hello PDF) Tj ET";
    add_obj(
        &mut pdf,
        &mut offsets,
        &format!(
            "4 0 obj\n<< /Length {} >>\nstream\n{}\nendstream\nendobj\n",
            stream.len(),
            stream
        ),
    );
    let xref_at = pdf.len();
    pdf.push_str("xref\n0 5\n0000000000 65535 f \n");
    for off in &offsets {
        pdf.push_str(&format!("{} 00000 n \n", pad10(*off)));
    }
    pdf.push_str(&format!(
        "trailer\n<< /Size 5 /Root 1 0 R >>\nstartxref\n{xref_at}\n%%EOF\n"
    ));
    pdf
}

fn main() -> anyhow::Result<()> {
    let dir = std::env::temp_dir();
    let src = format!("{}/rustscript_pdf_example_in.pdf", dir.display());
    let out = format!("{}/rustscript_pdf_example_out.pdf", dir.display());
    std::fs::write(&src, build_mini_pdf())?;

    let mut doc = Document::load(&src)?;
    let pages = doc.get_pages();
    println!("pages: {}", pages.len());

    let id = *pages.get(&1).unwrap();
    let content = String::from_utf8(doc.get_page_content(id))?;
    println!("content has greeting: {}", content.contains("Hello PDF"));

    let edited = content.replace("Hello PDF", "Edited PDF");
    doc.change_page_content(id, edited.into_bytes())?;
    doc.save(&out)?;

    let reloaded = Document::load(&out)?;
    let id = *reloaded.get_pages().get(&1).unwrap();
    let text = String::from_utf8(reloaded.get_page_content(id))?;
    println!("after edit: {}", text.contains("Edited PDF"));
    Ok(())
}
