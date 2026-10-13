// A `ref mut` binding of a `let else` over a place borrows the place like `if let` does.

#[derive(Debug)]
struct T(i64);

impl Drop for T {
    fn drop(&mut self) {
        println!("drop {}", self.0);
    }
}

fn bump(o: &mut Option<usize>) {
    let Some(ref mut r) = *o else {
        return;
    };
    *r += 100;
}

fn main() {
    let mut h: Option<usize> = Some(1);
    let Some(ref mut r) = h else {
        return;
    };
    *r += 1;
    println!("{h:?}");
    let Some(ref r) = h else {
        return;
    };
    println!("{r}");
    bump(&mut h);
    println!("{h:?}");
    let mut pair = (Some(T(1)), T(2));
    let (Some(ref mut t), ref other) = pair else {
        return;
    };
    t.0 += 10;
    println!("{:?} {:?}", t, other);
    println!("{pair:?}");
    let mut items = vec![Some(3u8), None];
    let Some(ref mut first) = items[0] else {
        return;
    };
    *first += 1;
    println!("{items:?}");
    let mut none: Option<T> = None;
    let Some(ref mut gone) = none else {
        println!("none");
        none = Some(T(7));
        println!("{none:?}");
        return;
    };
    gone.0 += 1;
}
