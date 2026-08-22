// `replace` with a char set pattern like `[':', '.']` must replace any
// member. The board tooling builds `Windows` safe filenames this way, so a
// no-op left illegal colons in the name.

fn main() {
    let stamp = "2026-07-23T15:43:33.672373+00:00";
    println!("{}", stamp.replace([':', '.'], "-"));
    println!("{}", stamp.replace([':', '.'], "-").replace('T', "_"));
    println!("{}", "a.b.c".replace('.', "/"));
    println!("{}", "foofoo".replacen("foo", "bar", 1));
    println!("{}", "a:b.c:d".replacen([':', '.'], "-", 2));
}
