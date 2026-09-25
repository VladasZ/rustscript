// With a `Drop` impl in the program, a fresh `zip` or `chain` argument is taken as an owning
// iterator. A `let` annotated `collect` inside a block argument still knows its type.
// Found by seed 20721116441.

struct T;

impl Drop for T {
    fn drop(&mut self) {}
}

fn main() {
    let w: Vec<(char, char)> = vec!['a']
        .into_iter()
        .zip({
            let s: Vec<char> = vec![1].into_iter().map(|_x: i32| ' ').collect();
            s
        })
        .collect();
    println!("{w:?}");
    let c: Vec<char> = vec!['b']
        .into_iter()
        .chain({
            let mut s: Vec<char> = vec![2, 1].into_iter().map(|x: i32| (b'a' + x as u8) as char).collect();
            s.sort();
            s
        })
        .collect();
    println!("{c:?}");
}
