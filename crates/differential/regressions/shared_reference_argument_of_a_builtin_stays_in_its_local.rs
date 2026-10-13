// written by hand, a builtin that hands a shared reference argument on leaves the local whole

fn main() {
    let mut v: String = String::from("");
    let mut i: usize = 0usize;
    while i < 3usize {
        i += 1usize;
        v.push_str(&String::from("a"));
        println!("{:?}", None::<&str>.unwrap_or(&v));
        println!("{:?}", None::<&str>.unwrap_or((&v)));
    }
    let w = vec![1, 2];
    println!("{:?}", None::<&Vec<i32>>.unwrap_or(&w));
    println!("{:?}", w);
}
