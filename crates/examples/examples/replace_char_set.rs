// str::replace / replacen with a char-set pattern like [':', '.'] must replace any member, the same as
// real Rust. The board tooling builds Windows-safe filenames this way, so a no-op here left illegal
// colons in the name. Single-char and string patterns must still work unchanged.

fn main() {
    let stamp = "2026-07-23T15:43:33.672373+00:00";
    println!("{}", stamp.replace([':', '.'], "-"));
    println!("{}", stamp.replace([':', '.'], "-").replace('T', "_"));
    println!("{}", "a.b.c".replace('.', "/"));
    println!("{}", "foofoo".replacen("foo", "bar", 1));
    println!("{}", "a:b.c:d".replacen([':', '.'], "-", 2));
}
