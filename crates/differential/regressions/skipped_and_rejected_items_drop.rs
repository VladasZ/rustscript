#[derive(Debug, Clone, PartialEq)]
struct T(i64);
impl Drop for T {
    fn drop(&mut self) {
        println!("drop {}", self.0);
    }
}

fn main() {
    let kept: Vec<T> = vec![T(1), T(2), T(3)]
        .into_iter()
        .enumerate()
        .skip_while(|(i, _)| *i < 2)
        .map(|(_, x)| x)
        .collect();
    println!("kept {kept:?}");
    let plain: Vec<T> = vec![T(4), T(5)].into_iter().skip_while(|x| x.0 < 5).collect();
    println!("plain {plain:?}");
    let found = vec![T(6), T(7), T(8)]
        .into_iter()
        .enumerate()
        .find_map(|(i, x)| if i == 1 { Some(x) } else { None });
    println!("found {found:?}");
    let none = vec![T(9)].into_iter().find_map(|x| if x.0 > 9 { Some(x) } else { None });
    println!("none {none:?}");
    let mut full = Some(T(10));
    let got = full.get_or_insert(T(11)).clone();
    println!("got {got:?}");
    let closure_unused: T = {
        let mut slot = Some(T(12));
        let spare = T(13);
        slot.get_or_insert_with(|| spare).clone()
    };
    println!("unused {closure_unused:?}");
    println!("end");
}
