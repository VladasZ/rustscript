fn main() {
    let mut current: Option<(String, String)> = Some(("a".to_string(), String::new()));
    for line in ["x", "y"] {
        if let Some((_, body)) = current.as_mut() {
            body.push_str(line);
            body.push('\n');
        }
    }
    println!("{current:?}");

    let mut single: Option<String> = Some(String::new());
    if let Some(s) = single.as_mut() {
        s.push('q');
    }
    println!("{single:?}");

    let mut count: Option<i32> = Some(1);
    if let Some(n) = count.as_mut() {
        *n += 1;
    }
    println!("{count:?}");

    let mut res: Result<(String, i32), String> = Ok(("r".to_string(), 0));
    if let Ok((name, n)) = res.as_mut() {
        name.push('!');
        *n += 5;
    }
    println!("{res:?}");

    let mut pair = ("a".to_string(), 1);
    let (text, n) = &mut pair;
    text.push('w');
    *n += 1;
    println!("{pair:?}");
}
