#!/usr/bin/env rust

#[derive(Debug)]
enum State {
    Todo,
    Doing,
    Done,
}

fn next(s: State) -> State {
    match s {
        State::Todo => State::Doing,
        State::Doing => State::Done,
        State::Done => State::Done,
    }
}

fn main() {
    let mut s = State::Todo;
    for _ in 0..4 {
        println!("{s:?}");
        s = next(s);
    }
}
