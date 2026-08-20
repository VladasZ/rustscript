// Every spelling of `Default::default()` the generator emits, a struct
// update that keeps declaration order, and `unwrap_or_default` reading the
// payload type off a turbofish, an annotation, a `None::<T>`, and a branch.
use std::collections::HashMap;

#[derive(Default, Debug, Clone, PartialEq)]
struct Inner {
    x: u8,
    tag: String,
}

#[derive(Default, Debug)]
struct Outer {
    a: Inner,
    b: (u8, char),
    c: Option<Inner>,
}

#[derive(Default, Debug, Clone)]
enum Mode {
    #[default]
    Idle,
    Busy(u32),
}

fn make() -> Option<Outer> {
    None
}

fn tail() -> (u8, String) {
    Default::default()
}

fn main() {
    let t: (i32, bool) = Default::default();
    println!("{:?} {:?} {t:?} {:?}", Vec::<u8>::default(), <(i32, bool, char)>::default(), u8::default());
    println!("{:?} {:?} {:?}", Inner::default(), <Mode>::default(), Mode::Busy(2));
    let outer = Outer { c: Some(Inner { x: 3, tag: String::from("set") }), ..Default::default() };
    println!("{outer:?} {:?}", tail());
    let m: HashMap<String, (Option<char>,)> = HashMap::new();
    let a = m.get("k").cloned().unwrap_or_default();
    let b = HashMap::<(i8, u16), (i32,)>::new().get(&(1, 2)).cloned().unwrap_or_default();
    let v: Vec<Mode> = Vec::new();
    let c = v.first().cloned().unwrap_or_default();
    let d = make().unwrap_or_default();
    let e: Vec<i32> = None.unwrap_or_default();
    let f = <Option<Vec<u32>>>::default().unwrap_or_default();
    let g = None::<(Option<i16>, bool)>.unwrap_or_default();
    let h = (if e.is_empty() { HashMap::<u32, (u8,)>::new() } else { HashMap::new() })
        .get(&1)
        .cloned()
        .unwrap_or_default();
    println!("{a:?} {b:?} {c:?} {d:?} {e:?} {f:?} {g:?} {h:?}");
}
