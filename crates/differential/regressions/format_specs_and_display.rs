// Format spec edges, including a user `Display` that pads only through `f.pad`.
use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt;

#[derive(Debug, Default)]
struct In {
    x: u8,
    s: String,
}

#[derive(Debug)]
enum E {
    Unit,
    Pair(i32, String),
}

struct W(i32);

impl fmt::Display for W {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "w{}", self.0)
    }
}

struct P(i32);

impl fmt::Display for P {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(&format!("p{}", self.0))
    }
}

fn main() {
    println!("[{:>6?}] [{:<8?}] [{:>6?}] [{:>8?}] [{:>5?}]", "", String::from("ab"), 'A', vec!["a"], Some("x"));
    println!("[{:>4?}] [{:.2?}] [{:>5?}] [{:+?}] [{:08.3?}]", vec![1, 22], vec![1.0f64, 2.345], true, Some(3), (1.5f64, -2.0f64));
    println!("[{:.*}] [{:~>0w$}] [{:_<+#7?}] [{:>4$}]", 2, 1.23456, 7, 2147483648u32, 5, w = 5);
    println!("[{:>6}] [{:<6}] [{:^7}] [{}]", W(1), P(2), P(3), W(4));
    let nested = (In { x: 1, s: String::from("q") }, vec![1.0, 2.5], E::Pair(4, String::from("p")), E::Unit);
    println!("{nested:#?}");
    let mut m: HashMap<u8, Vec<i32>> = HashMap::new();
    m.insert(1, vec![2]);
    println!("{m:#?} {:#?} {:#?} {:?}", Vec::<u8>::new(), Some(Some(1)), RefCell::new(5));
}
