#!/usr/bin/env rust

// The `String` methods that edit in place, `insert`, `insert_str`, `truncate`, `remove`, `pop`
// and `retain`. Each one must land on a local, a `&mut String` param, a struct field and a
// `RefCell`, and a clone taken before the edit keeps its own text.

use std::cell::RefCell;
use std::rc::Rc;

struct Doc {
    title: String,
}

fn shout(s: &mut String) -> Option<char> {
    s.insert(0, '!');
    s.insert_str(1, ">> ");
    s.pop()
}

fn main() {
    let mut s = String::from("hello world");
    let snapshot = s.clone();
    s.insert(5, ',');
    s.insert_str(0, "say: ");
    println!("{s}");
    println!("snapshot {snapshot}");

    let removed = s.remove(0);
    println!("removed {removed:?} left {s}");
    s.truncate(9);
    println!("truncated {s:?}");
    s.truncate(100);
    println!("past the end {s:?}");

    let mut letters = String::from("ab");
    while let Some(c) = letters.pop() {
        println!("popped {c}");
    }
    println!("pop on empty {:?}", letters.pop());

    let mut noisy = String::from("h-e-l-l-o w-o-r-l-d");
    noisy.retain(|c| c != '-');
    println!("{noisy}");
    let mut vowels = 0;
    noisy.retain(|c| {
        let keep = !"aeiou".contains(c);
        if !keep {
            vowels += 1;
        }
        keep
    });
    println!("{noisy} without {vowels} vowels");

    // byte offsets around a multibyte char
    let mut word = String::from("naïve");
    println!("removed {:?}", word.remove(2));
    word.insert(2, 'ï');
    word.insert_str(word.len(), "!?");
    println!("{word} len {}", word.len());

    let mut id = String::from("abc");
    println!("shout popped {:?} into {id}", shout(&mut id));
    println!("{id}");

    let mut doc = Doc {
        title: String::from("draft title"),
    };
    doc.title.truncate(5);
    doc.title.insert_str(0, "[[");
    doc.title.push(']');
    println!("{}", doc.title);

    let shared = Rc::new(RefCell::new(String::from("shared text")));
    shared.borrow_mut().retain(|c| c != 'e');
    shared.borrow_mut().insert(0, '>');
    let last = shared.borrow_mut().pop();
    println!("{} last {last:?}", shared.borrow());
}
