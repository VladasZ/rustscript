//! `?` on an error the function already returns is identity. The interpreter
//! once ran an unrelated `From` impl. From seed 140079.

#[derive(Debug)]
enum Failure {
    Plain(bool),
    Parsed(std::num::ParseIntError),
}

impl From<bool> for Failure {
    fn from(value: bool) -> Self {
        Self::Plain(value)
    }
}

impl From<std::num::ParseIntError> for Failure {
    fn from(value: std::num::ParseIntError) -> Self {
        Self::Parsed(value)
    }
}

fn already_typed() -> Result<bool, Failure> {
    Ok(Err::<bool, Failure>(Failure::Plain(false))?)
}

fn converts() -> Result<bool, Failure> {
    Ok(Err::<bool, bool>(true)?)
}

fn parses() -> Result<i32, Failure> {
    Ok("x".parse::<i32>()?)
}

fn main() {
    println!("{:?}", already_typed());
    println!("{:?}", converts());
    println!("{:?}", parses());
}
