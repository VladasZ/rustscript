#[derive(Debug)]
struct Broken(i32);

fn main() {
    let fine: Result<u8, String> = Ok(1);
    println!("{}", fine.expect("fine"));
    let some: Option<u8> = Some(2);
    println!("{}", some.expect("fine"));
    let broken: Result<u8, Broken> = Err(Broken(3));
    println!("{}", broken.expect("needs a value"));
}
