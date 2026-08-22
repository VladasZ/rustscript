#!/usr/bin/env rust

// The interpreter once treated a clone as a refcount bump or a one level
// copy, so mutating the clone leaked into the original.

#[derive(Clone, Debug)]
struct Basket {
    label: String,
    items: Vec<i32>,
}

#[derive(Clone, Debug)]
enum Load {
    Boxes(Vec<i32>),
    Empty,
}

fn stuff(mut basket: Basket) -> usize {
    basket.items.push(99);
    basket.label.push('!');
    basket.items.len()
}

fn main() {
    let a = Basket {
        label: "fruit".to_string(),
        items: vec![1, 2],
    };
    let mut b = a.clone();
    b.items.push(3);
    b.label.push_str("-basket");
    println!("{a:?}");
    println!("{b:?}");

    let filled = stuff(a.clone());
    println!("{filled} {a:?}");

    let load = Load::Boxes(vec![10]);
    let copy = load.clone();
    if let Load::Boxes(mut boxes) = copy {
        boxes.push(20);
        println!("{boxes:?}");
    }
    println!("{load:?}");
    let empty = Load::Empty;
    println!("{empty:?}");

    let nested = vec![vec![1], vec![2]];
    let mut twin = nested.clone();
    twin[0].push(5);
    println!("{nested:?} {twin:?}");

    let mut map = std::collections::HashMap::new();
    map.insert("k".to_string(), vec![1]);
    let mut copy = map.clone();
    copy.get_mut("k").unwrap().push(2);
    println!("{:?} {:?}", map["k"], copy["k"]);
}
