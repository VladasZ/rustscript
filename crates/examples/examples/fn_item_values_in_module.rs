// A bare function item as a value, named inside the module that declares it and from the
// module above it.

mod words {
    fn is_long(text: &str) -> bool {
        text.len() > 3
    }

    pub fn shout(text: &str) -> String {
        text.to_uppercase()
    }

    pub fn check(word: Option<String>) -> bool {
        word.as_deref().is_some_and(is_long)
    }

    pub fn long_ones(all: &[&str]) -> Vec<String> {
        all.iter()
            .copied()
            .filter(|text| is_long(text))
            .map(shout)
            .collect()
    }

    pub mod inner {
        pub fn twice(x: i64) -> i64 {
            x * 2
        }

        pub fn all(nums: Vec<i64>) -> Vec<i64> {
            nums.into_iter().map(twice).collect()
        }
    }
}

use words::shout;

fn main() {
    println!("{}", words::check(Some("hello".to_string())));
    println!("{}", words::check(Some("hi".to_string())));
    println!("{}", words::check(None));
    println!("{:?}", words::long_ones(&["one", "three", "five"]));
    println!("{:?}", words::inner::all(vec![1, 2, 3]));

    // named by path and by import from the root
    let loud: Vec<String> = vec!["a", "b"].into_iter().map(words::shout).collect();
    println!("{loud:?}");
    let some: Option<&str> = Some("x");
    println!("{:?}", some.map(shout));
    let f = words::inner::twice;
    println!("{}", f(21));
}
