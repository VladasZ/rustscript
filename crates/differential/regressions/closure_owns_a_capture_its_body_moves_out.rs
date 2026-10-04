#[derive(Debug, Clone, PartialEq)]
struct T(i64);
impl Drop for T {
    fn drop(&mut self) {
        println!("drop {}", self.0);
    }
}
fn take(t: T) -> i64 {
    t.0
}
fn main() {
    let x: T = {
        let r = None::<T>;
        let d = T(8);
        r.unwrap_or_else(move || d)
    };
    println!("M1 {x:?}");
    let y: T = {
        let r = Some(T(9));
        let d = T(10);
        r.unwrap_or_else(move || d)
    };
    println!("M2 {y:?}");
    {
        let d = T(20);
        let f = || d;
        println!("N1 made");
        let got = f();
        println!("N1 {got:?}");
    }
    {
        let d = T(30);
        let f = || d;
        println!("N2 made, never called");
        drop(f);
        println!("N2 dropped");
    }
    {
        let d = T(40);
        let e = T(41);
        let f = || take(d) + e.0;
        println!("N3 {}", f());
        println!("N3 e {e:?}");
    }
    {
        let d = T(50);
        let f = |n: i64| {
            let inner = d;
            inner.0 + n
        };
        println!("N4 {}", f(1));
    }
}
