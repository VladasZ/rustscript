// `Self::Variant(x)`, `?` through `From`, and a `From` picked by the argument type.
#[derive(Debug)]
enum Inner {
    Bad(String),
}

#[derive(Debug)]
enum E {
    Parse(std::num::ParseIntError),
    Wrapped(Inner),
    Neg,
}

impl From<std::num::ParseIntError> for E {
    fn from(e: std::num::ParseIntError) -> Self {
        Self::Parse(e)
    }
}

impl From<Inner> for E {
    fn from(e: Inner) -> Self {
        Self::Wrapped(e)
    }
}

impl From<String> for Inner {
    fn from(s: String) -> Self {
        Self::Bad(s)
    }
}

impl E {
    fn neg() -> Self {
        Self::Neg
    }
}

fn inner(s: &str) -> Result<i32, Inner> {
    if s.is_empty() {
        Err(Inner::Bad(String::from("empty")))
    } else {
        Ok(s.len() as i32)
    }
}

fn f(s: &str) -> Result<i32, E> {
    let v = s.parse::<i32>()?;
    if v < 0 {
        return Err(E::neg());
    }
    let n = inner(s)?;
    Ok(v + n)
}

fn g(s: &str) -> Result<i32, E> {
    let n = inner(s)?;
    Ok(n)
}

fn main() {
    println!("{:?} {:?} {:?} {:?}", f("x"), f("-1"), f("3"), g(""));
    let a: Inner = String::from("s").into();
    let b: E = Inner::Bad(String::from("t")).into();
    println!("{a:?} {b:?}");
    println!("{:?} {:?}", E::from("q".parse::<i32>().unwrap_err()), E::from(Inner::Bad(String::from("u"))));
}
