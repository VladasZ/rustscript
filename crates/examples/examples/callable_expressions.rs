fn make_offset(delta: i64) -> impl Fn(i64) -> i64 {
    move |value| value.saturating_add(delta)
}

fn main() {
    let from_returned_closure = make_offset(3)(7);

    let operation = make_offset(-2);
    let from_parenthesized_closure = (operation)(10);

    let operations = (make_offset(4), make_offset(-4));
    let from_tuple_field = (operations.1)(10);

    println!("{from_returned_closure}:{from_parenthesized_closure}:{from_tuple_field}");
}
