//! The peeked item must still be first in `collect`.

fn main() {
    let mut parts = "a,b,,c".split(',').peekable();
    while let Some(part) = parts.next() {
        let sep = if parts.peek().is_some() { ";" } else { "." };
        print!("{part}{sep}");
    }
    println!();

    let mut fields = "x=1;y=2".split(';').peekable();
    println!("first is {:?}", fields.peek());
    let collected: Vec<&str> = fields.collect();
    println!("{collected:?}");
}
