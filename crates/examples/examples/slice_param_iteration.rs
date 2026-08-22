//! A `&[T]` parameter forwards the caller's storage. Iterating it with a plain
//! `for` must borrow, not own, or the loop drains the elements out from under
//! the caller and every later read of that vec comes back empty.

struct Sample {
    value: f64,
}

fn sum_slice(items: &[Sample]) -> f64 {
    let mut total = 0.0;
    for item in items {
        total += item.value;
    }
    total
}

fn count_vec_ref(items: &Vec<i64>) -> usize {
    let mut seen = 0;
    for item in items {
        if *item > 0 {
            seen += 1;
        }
    }
    seen
}

fn main() {
    let samples = vec![
        Sample { value: 1.5 },
        Sample { value: 2.5 },
        Sample { value: 3.0 },
    ];
    println!("len before {}", samples.len());
    println!("sum first  {}", sum_slice(&samples));
    println!("len after  {}", samples.len());
    println!("sum again  {}", sum_slice(&samples));

    let numbers = vec![1_i64, -2, 3, -4, 5];
    println!("count first {}", count_vec_ref(&numbers));
    println!("count again {}", count_vec_ref(&numbers));
    println!("numbers len {}", numbers.len());

    // a local borrow keeps working the same way
    let borrowed = &samples;
    let mut local = 0.0;
    for item in borrowed {
        local += item.value;
    }
    println!("local sum  {local}");
    println!("len at end {}", samples.len());
}
