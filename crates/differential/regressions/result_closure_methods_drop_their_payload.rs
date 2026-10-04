#[derive(Debug, Clone, PartialEq)]
struct T(i64);
impl Drop for T {
    fn drop(&mut self) {
        println!("drop {}", self.0);
    }
}
fn main() {
    let ok: Result<T, T> = Ok(T(1));
    let a = ok.map_or(T(2), |x| x);
    println!("a {a:?}");
    let err: Result<T, T> = Err(T(3));
    let b = err.map_or(T(4), |x| x);
    println!("b {b:?}");
    let err: Result<T, T> = Err(T(5));
    let c = { let d = T(6); err.map_or_else(|_| d, |x| x) };
    println!("c {c:?}");
    let err: Result<T, T> = Err(T(7));
    let d = { let d = T(8); err.unwrap_or_else(|_| d) };
    println!("d {d:?}");
    let ok: Result<T, T> = Ok(T(9));
    let e = { let d = T(10); ok.unwrap_or_else(|_| d) };
    println!("e {e:?}");
    let ok: Result<T, T> = Ok(T(11));
    println!("f {}", ok.is_ok_and(|x| x.0 > 0));
    let err: Result<T, T> = Err(T(12));
    println!("g {}", err.is_err_and(|x| x.0 > 0));
    let err: Result<T, T> = Err(T(13));
    println!("h {}", err.is_ok_and(|x| x.0 > 0));
    let s = Some(T(14));
    println!("i {}", s.is_some_and(|x| x.0 > 0));
    let s = Some(T(15));
    let j = s.map_or_else(|| T(16), |x| x);
    println!("j {j:?}");
    let s: Option<T> = None;
    let k = { let d = T(17); s.ok_or_else(|| d) };
    println!("k {k:?}");
    let s = Some(T(18));
    let l = { let d = T(19); s.or_else(|| Some(d)) };
    println!("l {l:?}");
    let mut o = Some(T(20));
    let m = o.get_or_insert(T(21)).clone();
    println!("m {m:?}");
    let ok: Result<T, T> = Ok(T(22));
    println!("n {:?}", ok.as_ref().ok().cloned());
    println!("end");
}
