// `is_char_boundary` returns a bool, and the interpreter's type inference must know that. When its
// result feeds `then_some(...).unwrap_or_default()`, a missing type for the bool left the option
// payload unknown, so `unwrap_or_default` built an empty string instead of the enum default.

#[derive(Debug, Clone, PartialEq, Default)]
enum Marker {
    #[default]
    First,
    Second,
}

fn main() {
    let text = String::from("héllo");

    println!("{}", text.is_char_boundary(0));
    println!("{}", text.is_char_boundary(1));
    println!("{}", text.is_char_boundary(2));

    // the false branch drops to the enum default, the shape the differential campaign found. The
    // option is bound first so the type flows through it, the same path the bug broke.
    let dropped = text.is_char_boundary(20).then_some(Marker::Second);
    let picked = dropped.unwrap_or_default();
    println!("{picked:?}");

    let held = text.is_char_boundary(1).then_some(Marker::Second);
    let kept = held.unwrap_or_default();
    println!("{kept:?}");
}
