// `E::from(pick(..))` must pick the impl for what `pick` returns. The expected payload of one
// `From` impl used to bind the generic first and win over the argument. Seeds 20689212018,
// 20691204075, 20691204076.

#[derive(Debug)]
enum DiffErr {
    V2(usize),
    Parse(std::num::ParseIntError),
}

impl From<usize> for DiffErr {
    fn from(value: usize) -> Self {
        Self::V2(value)
    }
}

impl From<std::num::ParseIntError> for DiffErr {
    fn from(value: std::num::ParseIntError) -> Self {
        Self::Parse(value)
    }
}

fn diff_opaque_u64(value: u64) -> u64 {
    value
}

fn diff_pick<T: Clone + std::fmt::Debug>(a: T, b: T, first: bool) -> T {
    if first { a } else { b }
}

fn diff_one<T>(a: T) -> T {
    a
}

fn main() {
    println!("{:?}", DiffErr::from(diff_pick((diff_opaque_u64(9788397491860965012) as usize).wrapping_shr(3), (diff_opaque_u64(9223372036854775808) as usize).saturating_sub(1), false)));
    println!("{:?}", DiffErr::from(diff_one(diff_opaque_u64(5) as usize)));
    println!("{:?}", DiffErr::from("x".parse::<i32>().unwrap_err()));
    println!("{:?}", DiffErr::from(diff_one("x".parse::<i32>().unwrap_err())));
}
